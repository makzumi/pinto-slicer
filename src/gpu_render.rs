//! Offscreen wgpu viewport. `View` remains the camera's only source of truth;
//! meshes, lines and the background are rendered into an egui-native texture.

use bytemuck::{Pod, Zeroable};
use eframe::egui;
use std::collections::HashMap;

use crate::mesh::{Mesh, Vec3};
use crate::render::{Scene, View, NEAR};

const SAMPLES: u32 = 4;
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const BG_TOP: [u8; 3] = [24, 27, 33];
const BG_BOTTOM: [u8; 3] = [10, 12, 15];

/// Column-major world-to-clip matrix with an infinite-far reverse-Z mapping.
/// After perspective division, NDC depth is `NEAR / camera_z`: one at the near
/// plane and approaching zero in the distance. X/Y exactly match `View`.
///
/// An orthographic view has no divide to do it with, so it clips with `w = 1`
/// and walks depth in a straight line instead: one at the near plane, zero at
/// the view's far plane.
pub fn view_projection(view: &View) -> [f32; 16] {
    let sx = 2.0 * view.focal / view.w;
    let sy = 2.0 * view.focal / view.h;
    let r = view.right;
    let u = view.up;
    let f = view.fwd;
    let e = view.eye;
    if view.ortho {
        let span = (view.depth_far - NEAR).max(NEAR);
        let k = 1.0 / span;
        return [
            r.x * sx,
            u.x * sy,
            -f.x * k,
            0.0,
            r.y * sx,
            u.y * sy,
            -f.y * k,
            0.0,
            r.z * sx,
            u.z * sy,
            -f.z * k,
            0.0,
            -e.dot(r) * sx,
            -e.dot(u) * sy,
            (e.dot(f) + view.depth_far) * k,
            1.0,
        ];
    }
    [
        r.x * sx,
        u.x * sy,
        0.0,
        f.x,
        r.y * sx,
        u.y * sy,
        0.0,
        f.y,
        r.z * sx,
        u.z * sy,
        0.0,
        f.z,
        -e.dot(r) * sx,
        -e.dot(u) * sy,
        NEAR,
        -e.dot(f),
    ]
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneRevision {
    pub models: u64,
    pub derived: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum MeshKey {
    Model(usize, u64),
    Support(u64),
    CutCap(u64, usize),
    Overhang(u64),
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

impl GpuMesh {
    fn new(device: &wgpu::Device, mesh: &Mesh) -> Self {
        let positions: Vec<[f32; 3]> = mesh.verts.iter().map(|v| [v.x, v.y, v.z]).collect();
        Self::from_parts(device, &positions, &mesh.tris)
    }

    fn from_parts(device: &wgpu::Device, positions: &[[f32; 3]], indices: &[[u32; 3]]) -> Self {
        use wgpu::util::DeviceExt;
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("resin mesh vertices"),
            contents: bytemuck::cast_slice(positions),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("resin mesh indices"),
            contents: bytemuck::cast_slice(indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Self {
            vertices,
            indices: index_buffer,
            index_count: (indices.len() * 3) as u32,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CameraUniform {
    matrix: [f32; 16],
    eye: [f32; 4],
    misc: [f32; 4],
    viewport: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DrawUniform {
    base: [f32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LineInstance {
    a: [f32; 3],
    b: [f32; 3],
    color: [f32; 4],
    width: f32,
    bias: f32,
}

impl LineInstance {
    fn new(a: Vec3, b: Vec3, rgb: [u8; 3], width: f32, bias: f32) -> Self {
        Self {
            a: [a.x, a.y, a.z],
            b: [b.x, b.y, b.z],
            color: color(rgb),
            width: width.max(1.0),
            bias,
        }
    }
}

struct GpuLineBuffer {
    buffer: wgpu::Buffer,
    count: u32,
}

impl GpuLineBuffer {
    fn new(device: &wgpu::Device, instances: &[LineInstance]) -> Self {
        use wgpu::util::DeviceExt;
        Self {
            buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("resin line instances"),
                contents: bytemuck::cast_slice(instances),
                usage: wgpu::BufferUsages::VERTEX,
            }),
            count: instances.len() as u32,
        }
    }
}

struct SliceLines {
    revision: u64,
    current_layer: usize,
    cross_section: bool,
    other: Option<GpuLineBuffer>,
    current: Option<GpuLineBuffer>,
}

struct Target {
    size: [u32; 2],
    _msaa: wgpu::Texture,
    msaa_view: wgpu::TextureView,
    #[allow(dead_code)] // owns the resolve texture for optional readback
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    _depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    egui_id: Option<egui::TextureId>,
}

impl Target {
    fn new(device: &wgpu::Device, size: [u32; 2]) -> Self {
        let size = [size[0].max(1), size[1].max(1)];
        let extent = wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        };
        let texture = |label, sample_count, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let msaa = texture(
            "resin viewport msaa",
            SAMPLES,
            TARGET_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let color = texture(
            "resin viewport resolve",
            1,
            TARGET_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
        );
        let depth = texture(
            "resin viewport depth",
            SAMPLES,
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let msaa_view = msaa.create_view(&Default::default());
        let color_view = color.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());
        Self {
            size,
            _msaa: msaa,
            msaa_view,
            color,
            color_view,
            _depth: depth,
            depth_view,
            egui_id: None,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BackgroundUniform {
    top: [f32; 4],
    bottom: [f32; 4],
    size: [f32; 4],
}

pub struct GpuRenderer {
    background_pipeline: wgpu::RenderPipeline,
    background_layout: wgpu::BindGroupLayout,
    background_uniform: wgpu::Buffer,
    mesh_pipeline: wgpu::RenderPipeline,
    camera_layout: wgpu::BindGroupLayout,
    camera_uniform: wgpu::Buffer,
    draw_layout: wgpu::BindGroupLayout,
    line_pipeline: wgpu::RenderPipeline,
    slice_lines: Option<SliceLines>,
    slice_rebuilds: u64,
    meshes: HashMap<MeshKey, GpuMesh>,
    upload_count: u64,
    target: Option<Target>,
}

impl GpuRenderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("resin background shader"),
            source: wgpu::ShaderSource::Wgsl(BACKGROUND_SHADER.into()),
        });
        let background_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("resin background layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("resin background pipeline layout"),
            bind_group_layouts: &[&background_layout],
            push_constant_ranges: &[],
        });
        let background_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("resin background pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: SAMPLES,
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: TARGET_FORMAT,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview: None,
            cache: None,
        });
        let background_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("resin background uniform"),
            size: std::mem::size_of::<BackgroundUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("resin camera layout"),
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
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("resin draw layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<DrawUniform>() as u64
                    ),
                },
                count: None,
            }],
        });
        let camera_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("resin camera uniform"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mesh_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("resin mesh shader"),
            source: wgpu::ShaderSource::Wgsl(MESH_SHADER.into()),
        });
        let mesh_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("resin mesh pipeline layout"),
            bind_group_layouts: &[&camera_layout, &draw_layout],
            push_constant_ranges: &[],
        });
        let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("resin mesh pipeline"),
            layout: Some(&mesh_layout),
            vertex: wgpu::VertexState {
                module: &mesh_shader,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 12,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x3,
                        offset: 0,
                        shader_location: 0,
                    }],
                }],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::GreaterEqual,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: SAMPLES,
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &mesh_shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: TARGET_FORMAT,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview: None,
            cache: None,
        });
        let line_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("resin line shader"),
            source: wgpu::ShaderSource::Wgsl(LINE_SHADER.into()),
        });
        let line_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("resin line pipeline layout"),
            bind_group_layouts: &[&camera_layout],
            push_constant_ranges: &[],
        });
        let line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("resin line pipeline"),
            layout: Some(&line_layout),
            vertex: wgpu::VertexState {
                module: &line_shader,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<LineInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 2,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 12,
                            shader_location: 3,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 24,
                            shader_location: 4,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32,
                            offset: 40,
                            shader_location: 5,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32,
                            offset: 44,
                            shader_location: 6,
                        },
                    ],
                }],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::GreaterEqual,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: SAMPLES,
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &line_shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: TARGET_FORMAT,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview: None,
            cache: None,
        });
        Self {
            background_pipeline,
            background_layout,
            background_uniform,
            mesh_pipeline,
            camera_layout,
            camera_uniform,
            draw_layout,
            line_pipeline,
            slice_lines: None,
            slice_rebuilds: 0,
            meshes: HashMap::new(),
            upload_count: 0,
            target: None,
        }
    }

    fn ensure_target(&mut self, device: &wgpu::Device, size: [u32; 2]) -> Option<egui::TextureId> {
        let size = [size[0].max(1), size[1].max(1)];
        if self.target.as_ref().is_some_and(|t| t.size == size) {
            return None;
        }
        let retired = self.target.take().and_then(|target| target.egui_id);
        self.target = Some(Target::new(device, size));
        retired
    }

    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &Scene,
        view: &View,
        size_px: [u32; 2],
        revision: SceneRevision,
    ) {
        let _ = self.ensure_target(device, size_px);
        let target = self.target.as_ref().expect("target was just created");
        let height = scene
            .slice
            .as_ref()
            .map(|slice| slice.height)
            .unwrap_or(0.0)
            .max(scene.build[2]);
        let camera_uniform = CameraUniform {
            matrix: view_projection(view),
            eye: [view.eye.x, view.eye.y, view.eye.z, 1.0],
            misc: [height, 0.0, 0.0, 0.0],
            viewport: [target.size[0] as f32, target.size[1] as f32, 0.0, 0.0],
        };
        queue.write_buffer(&self.camera_uniform, 0, bytemuck::bytes_of(&camera_uniform));
        let camera = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("resin camera bind group"),
            layout: &self.camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.camera_uniform.as_entire_binding(),
            }],
        });
        let uniform = BackgroundUniform {
            top: color(BG_TOP),
            bottom: color(BG_BOTTOM),
            size: [target.size[0] as f32, target.size[1] as f32, 0.0, 0.0],
        };
        queue.write_buffer(&self.background_uniform, 0, bytemuck::bytes_of(&uniform));
        let background = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("resin background bind group"),
            layout: &self.background_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.background_uniform.as_entire_binding(),
            }],
        });

        let mut calls = Vec::new();
        let mut draw_uniforms = Vec::new();
        let mut dynamics: Vec<(GpuMesh, u32)> = Vec::new();
        let dynamic_mesh = |positions: Vec<[f32; 3]>,
                            indices: Vec<[u32; 3]>,
                            base: [u8; 3],
                            dynamics: &mut Vec<(GpuMesh, u32)>,
                            draw_uniforms: &mut Vec<DrawUniform>| {
            if indices.is_empty() {
                return;
            }
            let uniform_index = draw_uniforms.len() as u32;
            draw_uniforms.push(DrawUniform {
                base: color(base),
                params: [0.0, 0.0, height, 0.0],
            });
            dynamics.push((
                GpuMesh::from_parts(device, &positions, &indices),
                uniform_index,
            ));
        };
        if scene.show_plate {
            let hx = scene.build[0] * 0.5;
            let hy = scene.build[1] * 0.5;
            dynamic_mesh(
                vec![
                    [-hx, -hy, -0.12],
                    [hx, -hy, -0.12],
                    [hx, hy, -0.12],
                    [-hx, hy, -0.12],
                ],
                vec![[0, 1, 2], [0, 2, 3]],
                [36, 40, 48],
                &mut dynamics,
                &mut draw_uniforms,
            );
        }
        if scene.mode == crate::render::ViewMode::Solid {
            let shades = [[33, 37, 44], [30, 34, 41], [27, 31, 38], [25, 28, 35]];
            for (level, base) in shades.into_iter().enumerate() {
                let scale = 1.0 - 0.16 * (level as f32 + 1.0);
                let mut positions = Vec::new();
                let mut indices = Vec::new();
                for mesh in scene.models {
                    if mesh.tris.is_empty() {
                        continue;
                    }
                    let (min, max) = mesh.bounds();
                    let centre = [(min.x + max.x) * 0.5, (min.y + max.y) * 0.5, 0.02];
                    let rx = ((max.x - min.x) * 0.5 + 1.0) * scale;
                    let ry = ((max.y - min.y) * 0.5 + 1.0) * scale;
                    let centre_index = positions.len() as u32;
                    positions.push(centre);
                    for step in 0..16 {
                        let angle = step as f32 / 16.0 * std::f32::consts::TAU;
                        positions.push([
                            centre[0] + rx * angle.cos(),
                            centre[1] + ry * angle.sin(),
                            centre[2],
                        ]);
                    }
                    for step in 0..16 {
                        indices.push([
                            centre_index,
                            centre_index + 1 + step,
                            centre_index + 1 + (step + 1) % 16,
                        ]);
                    }
                }
                dynamic_mesh(positions, indices, base, &mut dynamics, &mut draw_uniforms);
            }
        }
        let cut = scene.cut.unwrap_or(0.0);
        if scene.mode == crate::render::ViewMode::Solid {
            for (index, mesh) in scene.models.iter().enumerate() {
                if mesh.tris.is_empty() {
                    continue;
                }
                let key = MeshKey::Model(index, revision.models);
                if let std::collections::hash_map::Entry::Vacant(entry) = self.meshes.entry(key) {
                    entry.insert(GpuMesh::new(device, mesh));
                    self.upload_count += 1;
                }
                let uniform_index = draw_uniforms.len() as u32;
                let picked = scene.selection.contains(&index);
                draw_uniforms.push(DrawUniform {
                    base: color(if picked {
                        crate::render::PIECE_PICKED
                    } else {
                        crate::render::PIECE
                    }),
                    params: [1.0, 0.0, height, cut],
                });
                calls.push((key, uniform_index));
            }
        }
        if let Some(mesh) = scene.support_mesh.filter(|mesh| !mesh.tris.is_empty()) {
            let key = MeshKey::Support(revision.derived);
            if let std::collections::hash_map::Entry::Vacant(entry) = self.meshes.entry(key) {
                entry.insert(GpuMesh::new(device, mesh));
                self.upload_count += 1;
            }
            let uniform_index = draw_uniforms.len() as u32;
            draw_uniforms.push(DrawUniform {
                base: color([158, 164, 176]),
                params: [1.0, 1.0, height, cut],
            });
            calls.push((key, uniform_index));
        }
        if let Some(mesh) = scene.cut_cap.filter(|mesh| !mesh.tris.is_empty()) {
            let key = MeshKey::CutCap(revision.derived, scene.current_layer);
            if let std::collections::hash_map::Entry::Vacant(entry) = self.meshes.entry(key) {
                entry.insert(GpuMesh::new(device, mesh));
                self.upload_count += 1;
            }
            let uniform_index = draw_uniforms.len() as u32;
            draw_uniforms.push(DrawUniform {
                // A cut face has to read as filled material. The old dark
                // brown cap read as a hole at the top of every support.
                base: color([205, 197, 188]),
                params: [0.0, 0.0, height, 0.0],
            });
            calls.push((key, uniform_index));
        }
        if let Some(mesh) = scene.overhang.filter(|mesh| !mesh.tris.is_empty()) {
            let key = MeshKey::Overhang(revision.derived);
            if let std::collections::hash_map::Entry::Vacant(entry) = self.meshes.entry(key) {
                entry.insert(GpuMesh::new(device, mesh));
                self.upload_count += 1;
            }
            let uniform_index = draw_uniforms.len() as u32;
            draw_uniforms.push(DrawUniform {
                base: color([226, 74, 66]),
                params: [0.0, 0.0, height, 0.0],
            });
            calls.push((key, uniform_index));
        }
        self.meshes.retain(|key, _| match key {
            MeshKey::Model(..) => calls.iter().any(|(wanted, _)| wanted == key),
            MeshKey::Support(version) => {
                scene.support_mesh.is_some() && *version == revision.derived
            }
            MeshKey::CutCap(version, layer) => {
                scene.cut_cap.is_some()
                    && *version == revision.derived
                    && *layer == scene.current_layer
            }
            MeshKey::Overhang(version) => scene.overhang.is_some() && *version == revision.derived,
        });
        let alignment = device.limits().min_uniform_buffer_offset_alignment as usize;
        let mut bytes = vec![0_u8; draw_uniforms.len().max(1) * alignment];
        for (i, uniform) in draw_uniforms.iter().enumerate() {
            bytes[i * alignment..i * alignment + std::mem::size_of::<DrawUniform>()]
                .copy_from_slice(bytemuck::bytes_of(uniform));
        }
        let draw_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("resin draw uniforms"),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&draw_buffer, 0, &bytes);
        let draw_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("resin draw bind group"),
            layout: &self.draw_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &draw_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<DrawUniform>() as u64),
                }),
            }],
        });
        let mut plate_lines = Vec::new();
        if scene.show_plate {
            let hx = scene.build[0] * 0.5;
            let hy = scene.build[1] * 0.5;
            let z = -0.06;
            let mut x = -hx;
            while x <= hx + 0.01 {
                let major = (x.round() as i32) % 50 == 0;
                plate_lines.push(LineInstance::new(
                    Vec3::new(x, -hy, z),
                    Vec3::new(x, hy, z),
                    if major { [66, 73, 86] } else { [50, 56, 68] },
                    if major { 1.2 } else { 1.0 },
                    1.001,
                ));
                x += 10.0;
            }
            let mut y = -hy;
            while y <= hy + 0.01 {
                let major = (y.round() as i32) % 50 == 0;
                plate_lines.push(LineInstance::new(
                    Vec3::new(-hx, y, z),
                    Vec3::new(hx, y, z),
                    if major { [66, 73, 86] } else { [50, 56, 68] },
                    if major { 1.2 } else { 1.0 },
                    1.001,
                ));
                y += 10.0;
            }
            for (a, b) in [
                (Vec3::new(-hx, -hy, z), Vec3::new(hx, -hy, z)),
                (Vec3::new(hx, -hy, z), Vec3::new(hx, hy, z)),
                (Vec3::new(hx, hy, z), Vec3::new(-hx, hy, z)),
                (Vec3::new(-hx, hy, z), Vec3::new(-hx, -hy, z)),
            ] {
                plate_lines.push(LineInstance::new(a, b, [86, 133, 196], 1.6, 1.001));
            }
            plate_lines.push(LineInstance::new(
                Vec3::new(-4.0, 0.0, z),
                Vec3::new(4.0, 0.0, z),
                [190, 90, 80],
                1.2,
                1.001,
            ));
            plate_lines.push(LineInstance::new(
                Vec3::new(0.0, -4.0, z),
                Vec3::new(0.0, 4.0, z),
                [110, 180, 120],
                1.2,
                1.001,
            ));
        }
        for island in scene.islands {
            let radius = 1.1_f32;
            for (dx, dy) in [(1.0_f32, 0.0_f32), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)] {
                plate_lines.push(LineInstance::new(
                    Vec3::new(
                        island.x - dx * radius,
                        island.y - dy * radius,
                        island.z - 1.2,
                    ),
                    Vec3::new(
                        island.x + dx * radius,
                        island.y + dy * radius,
                        island.z + 1.2,
                    ),
                    [255, 208, 90],
                    1.6,
                    1.001,
                ));
            }
        }
        let plate_buffer =
            (!plate_lines.is_empty()).then(|| GpuLineBuffer::new(device, &plate_lines));
        let cross_section = scene.mode == crate::render::ViewMode::Solid && scene.cut.is_some();
        if scene.mode == crate::render::ViewMode::Sliced || cross_section {
            if let Some(slice) = scene.slice {
                let rebuild = self.slice_lines.as_ref().is_none_or(|cached| {
                    cached.revision != revision.derived
                        || cached.current_layer != scene.current_layer
                        || cached.cross_section != cross_section
                });
                if rebuild {
                    self.slice_rebuilds += 1;
                    let count = slice.layers.len();
                    let stride = (count / 60).max(1);
                    let shown: Vec<usize> = (0..count)
                        .filter(|i| *i == scene.current_layer || i % stride == 0)
                        .collect();
                    let total: usize = shown.iter().map(|i| slice.layers[*i].segs.len()).sum();
                    let seg_stride = (total / 60_000).max(1);
                    let mut other = Vec::new();
                    if !cross_section {
                        for &index in &shown {
                            if index == scene.current_layer {
                                continue;
                            }
                            let layer = &slice.layers[index];
                            for segment in layer.segs.iter().step_by(seg_stride) {
                                other.push(LineInstance::new(
                                    Vec3::new(segment.a[0], segment.a[1], layer.z),
                                    Vec3::new(segment.b[0], segment.b[1], layer.z),
                                    [122, 74, 74],
                                    1.0,
                                    1.001,
                                ));
                            }
                        }
                    }
                    let mut current = Vec::new();
                    if let Some(layer) = slice.layers.get(scene.current_layer) {
                        // In the 3D view the contour sits on the cut plane,
                        // not at the middle of the layer it came from.
                        let z = if cross_section {
                            scene.cut.unwrap_or(layer.z) + 0.01
                        } else {
                            layer.z
                        };
                        for segment in layer.segs.iter().step_by(seg_stride.clamp(1, 6)) {
                            current.push(LineInstance::new(
                                Vec3::new(segment.a[0], segment.a[1], z),
                                Vec3::new(segment.b[0], segment.b[1], z),
                                [233, 86, 74],
                                1.6,
                                1.15,
                            ));
                        }
                    }
                    self.slice_lines = Some(SliceLines {
                        revision: revision.derived,
                        current_layer: scene.current_layer,
                        cross_section,
                        other: (!other.is_empty()).then(|| GpuLineBuffer::new(device, &other)),
                        current: (!current.is_empty())
                            .then(|| GpuLineBuffer::new(device, &current)),
                    });
                }
            } else {
                self.slice_lines = None;
            }
        } else {
            self.slice_lines = None;
        }

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("resin viewport encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("resin viewport pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.msaa_view,
                    resolve_target: Some(&target.color_view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.background_pipeline);
            pass.set_bind_group(0, &background, &[]);
            pass.draw(0..3, 0..1);
            if !dynamics.is_empty() || !calls.is_empty() {
                pass.set_pipeline(&self.mesh_pipeline);
                pass.set_bind_group(0, &camera, &[]);
                for (mesh, uniform_index) in &dynamics {
                    pass.set_bind_group(1, &draw_bind, &[*uniform_index * alignment as u32]);
                    pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                    pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                }
                for (key, uniform_index) in calls {
                    let mesh = &self.meshes[&key];
                    pass.set_bind_group(1, &draw_bind, &[uniform_index * alignment as u32]);
                    pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                    pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                }
            }
            let mut line_buffers: Vec<&GpuLineBuffer> = Vec::new();
            if let Some(buffer) = &plate_buffer {
                line_buffers.push(buffer);
            }
            if let Some(cached) = &self.slice_lines {
                if let Some(buffer) = &cached.other {
                    line_buffers.push(buffer);
                }
                if let Some(buffer) = &cached.current {
                    line_buffers.push(buffer);
                }
            }
            if !line_buffers.is_empty() {
                pass.set_pipeline(&self.line_pipeline);
                pass.set_bind_group(0, &camera, &[]);
                for buffer in line_buffers {
                    pass.set_vertex_buffer(0, buffer.buffer.slice(..));
                    pass.draw(0..6, 0..buffer.count);
                }
            }
        }
        queue.submit([encoder.finish()]);
    }

    pub fn render(
        &mut self,
        state: &egui_wgpu::RenderState,
        scene: &Scene,
        view: &View,
        size_px: [u32; 2],
        revision: SceneRevision,
    ) -> egui::TextureId {
        let retired = self.ensure_target(&state.device, size_px);
        self.draw(&state.device, &state.queue, scene, view, size_px, revision);
        let target = self.target.as_mut().expect("target was just created");
        let mut renderer = state.renderer.write();
        if let Some(id) = retired {
            renderer.free_texture(&id);
        }
        if let Some(id) = target.egui_id {
            return id;
        }
        let id = renderer.register_native_texture(
            &state.device,
            &target.color_view,
            wgpu::FilterMode::Linear,
        );
        target.egui_id = Some(id);
        id
    }

    #[allow(dead_code)] // used by headless tests and GPU diagnostics
    pub fn read_rgba(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Vec<u8>> {
        let target = self.target.as_ref()?;
        let row_bytes = target.size[0] as usize * 4;
        let padded = (row_bytes + 255) & !255;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("resin viewport readback"),
            size: (padded * target.size[1] as usize) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            target.color.as_image_copy(),
            wgpu::ImageCopyBuffer {
                buffer: &buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded as u32),
                    rows_per_image: Some(target.size[1]),
                },
            },
            wgpu::Extent3d {
                width: target.size[0],
                height: target.size[1],
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        let _ = device.poll(wgpu::Maintain::Wait);
        rx.recv().ok()?.ok()?;
        let mapped = buffer.slice(..).get_mapped_range();
        let mut out = Vec::with_capacity(row_bytes * target.size[1] as usize);
        for row in mapped.chunks_exact(padded) {
            out.extend_from_slice(&row[..row_bytes]);
        }
        drop(mapped);
        buffer.unmap();
        Some(out)
    }
}

fn color(rgb: [u8; 3]) -> [f32; 4] {
    [
        rgb[0] as f32 / 255.0,
        rgb[1] as f32 / 255.0,
        rgb[2] as f32 / 255.0,
        1.0,
    ]
}

const LINE_SHADER: &str = r#"
fn srgb_channel(value: f32) -> f32 {
    if value <= 0.04045 {
        return value / 12.92;
    }
    return pow((value + 0.055) / 1.055, 2.4);
}

fn linearize(color: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(
        srgb_channel(color.r),
        srgb_channel(color.g),
        srgb_channel(color.b),
        color.a,
    );
}

struct Camera {
    matrix: mat4x4<f32>,
    eye: vec4<f32>,
    misc: vec4<f32>,
    viewport: vec4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;

struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

fn camera_z(point: vec3<f32>) -> f32 {
    return (camera.matrix * vec4<f32>(point, 1.0)).w;
}

fn clip_world(point: vec3<f32>) -> vec4<f32> {
    return camera.matrix * vec4<f32>(point, 1.0);
}

fn pixel(clip: vec4<f32>) -> vec2<f32> {
    let ndc = clip.xy / clip.w;
    return vec2<f32>(
        (ndc.x + 1.0) * camera.viewport.x * 0.5,
        (1.0 - ndc.y) * camera.viewport.y * 0.5,
    );
}

@vertex
fn vs_main(
    @builtin(vertex_index) index: u32,
    @location(2) instance_a: vec3<f32>,
    @location(3) instance_b: vec3<f32>,
    @location(4) instance_color: vec4<f32>,
    @location(5) instance_width: f32,
    @location(6) instance_bias: f32,
) -> LineOut {
    var out: LineOut;
    out.color = instance_color;
    var a = instance_a;
    var b = instance_b;
    let za = camera_z(a);
    let zb = camera_z(b);
    if za < 0.5 && zb < 0.5 {
        out.clip = vec4<f32>(0.0, 0.0, -1.0, 1.0);
        return out;
    }
    if za < 0.5 {
        a = mix(a, b, (0.5 - za) / (zb - za));
    }
    if zb < 0.5 {
        b = mix(b, a, (0.5 - zb) / (za - zb));
    }
    var ca = clip_world(a);
    var cb = clip_world(b);
    ca.z = min(ca.z * instance_bias, ca.w);
    cb.z = min(cb.z * instance_bias, cb.w);
    let pa = pixel(ca);
    let pb = pixel(cb);
    let delta = pb - pa;
    var normal = vec2<f32>(0.0);
    if length(delta) > 0.00001 {
        normal = normalize(vec2<f32>(-delta.y, delta.x));
    }
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
    );
    let corner = corners[index];
    let t = (corner.x + 1.0) * 0.5;
    let centre = mix(pa, pb, t);
    let screen = centre + normal * corner.y * max(instance_width, 1.0) * 0.5;
    let clip = mix(ca, cb, t);
    let ndc = vec2<f32>(
        screen.x / camera.viewport.x * 2.0 - 1.0,
        1.0 - screen.y / camera.viewport.y * 2.0,
    );
    out.clip = vec4<f32>(ndc * clip.w, clip.z, clip.w);
    return out;
}

@fragment
fn fs_main(in: LineOut) -> @location(0) vec4<f32> {
    return linearize(in.color);
}
"#;

const MESH_SHADER: &str = r#"
fn srgb_channel(value: f32) -> f32 {
    if value <= 0.04045 {
        return value / 12.92;
    }
    return pow((value + 0.055) / 1.055, 2.4);
}

fn linearize(color: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(
        srgb_channel(color.r),
        srgb_channel(color.g),
        srgb_channel(color.b),
        color.a,
    );
}

struct Camera {
    matrix: mat4x4<f32>,
    eye: vec4<f32>,
    misc: vec4<f32>,
    viewport: vec4<f32>,
};
struct Draw {
    base: vec4<f32>,
    params: vec4<f32>,
};
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<uniform> draw: Draw;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
};

@vertex
fn vs_main(@location(0) position: vec3<f32>) -> VertexOut {
    var out: VertexOut;
    out.clip = camera.matrix * vec4<f32>(position, 1.0);
    out.world = position;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // A non-zero params.w is the layer the 3D cross-section is cut at.
    if draw.params.w > 0.0 && in.world.z > draw.params.w {
        discard;
    }
    if draw.params.x < 0.5 {
        return linearize(draw.base);
    }
    var normal = normalize(cross(dpdx(in.world), dpdy(in.world)));
    let to_eye = normalize(camera.eye.xyz - in.world);
    if dot(normal, to_eye) < 0.0 {
        normal = -normal;
    }
    let key = normalize(vec3<f32>(0.45, -0.35, 0.82));
    let fill = normalize(vec3<f32>(-0.65, 0.4, 0.3));
    if draw.params.y > 0.5 {
        let light = max(dot(normal, key), 0.0) * 0.8
            + max(dot(normal, fill), 0.0) * 0.25 + 0.34;
        return linearize(vec4<f32>(min(draw.base.rgb * light, vec3<f32>(1.0)), draw.base.a));
    }
    let light = max(dot(normal, key), 0.0) * 0.9
        + max(dot(normal, fill), 0.0) * 0.3 + 0.26;
    let zf = select(clamp(in.world.z / draw.params.z, 0.0, 1.0), 0.5, draw.params.z <= 0.0);
    let tint = 0.9 + 0.16 * zf;
    return linearize(vec4<f32>(min(draw.base.rgb * light * tint, vec3<f32>(1.0)), draw.base.a));
}
"#;

const BACKGROUND_SHADER: &str = r#"
fn srgb_channel(value: f32) -> f32 {
    if value <= 0.04045 {
        return value / 12.92;
    }
    return pow((value + 0.055) / 1.055, 2.4);
}

fn linearize(color: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(
        srgb_channel(color.r),
        srgb_channel(color.g),
        srgb_channel(color.b),
        color.a,
    );
}

struct Background {
    top: vec4<f32>,
    bottom: vec4<f32>,
    size: vec4<f32>,
};
@group(0) @binding(0) var<uniform> background: Background;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    var points = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(points[index], 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let t = clamp(position.y / background.size.y, 0.0, 1.0);
    return linearize(mix(background.top, background.bottom, t));
}
"#;

/// Create a surface-free Vulkan device for diagnostics, benchmarks and tests.
pub fn headless_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..Default::default()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))?;
    pollster::block_on(adapter.request_device(&Default::default(), None)).ok()
}

#[cfg(test)]
fn transform_point(matrix: [f32; 16], p: Vec3) -> [f32; 4] {
    let v = [p.x, p.y, p.z, 1.0];
    let mut out = [0.0; 4];
    for (row, value) in out.iter_mut().enumerate() {
        *value = (0..4).map(|col| matrix[col * 4 + row] * v[col]).sum();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{Camera, ViewMode};

    #[test]
    fn the_gpu_matrix_matches_view_projection_to_a_hundredth_of_a_pixel() {
        for camera in [
            Camera::default(),
            Camera {
                yaw: 1.7,
                pitch: -0.35,
                dist: 85.0,
                target: Vec3::new(12.0, -8.0, 9.0),
                fov_deg: 52.0,
                ..Camera::default()
            },
            Camera {
                yaw: -2.4,
                pitch: 1.1,
                dist: 32.0,
                target: Vec3::new(-20.0, 4.0, 2.0),
                fov_deg: 35.0,
                ..Camera::default()
            },
            Camera {
                ortho: true,
                ..Camera::default()
            },
        ] {
            let view = View::new(&camera, 913.0, 617.0);
            let matrix = view_projection(&view);
            for offset in [
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(10.0, -4.0, 2.0),
                Vec3::new(-35.0, 22.0, 14.0),
                Vec3::new(70.0, 30.0, -8.0),
                Vec3::new(-90.0, -60.0, 50.0),
            ] {
                let point = camera.target + offset;
                let Some((want_x, want_y, camera_z)) = view.project(point) else {
                    continue;
                };
                let clip = transform_point(matrix, point);
                let x = (clip[0] / clip[3] + 1.0) * view.w * 0.5;
                let y = (1.0 - clip[1] / clip[3]) * view.h * 0.5;
                assert!((x - want_x).abs() < 0.01, "x {x} != {want_x}");
                assert!((y - want_y).abs() < 0.01, "y {y} != {want_y}");
                if view.ortho {
                    // No divide: the far plane has to be a real distance.
                    assert!((clip[3] - 1.0).abs() < 1e-6);
                    let want = (view.depth_far - camera_z) / (view.depth_far - NEAR);
                    assert!((clip[2] - want).abs() < 1e-5, "depth {} != {want}", clip[2]);
                } else {
                    assert!((clip[3] - camera_z).abs() < 1e-4);
                    assert!((clip[2] / clip[3] - NEAR / camera_z).abs() < 1e-5);
                }
            }
        }
    }

    #[test]
    fn the_offscreen_background_can_be_read_back() {
        let Some((device, queue)) = headless_device() else {
            eprintln!("no Vulkan adapter for the headless GPU test");
            return;
        };
        let mut renderer = GpuRenderer::new(&device);
        let scene = Scene {
            models: &[],
            selection: &[],
            support_mesh: None,
            cut_cap: None,
            overhang: None,
            islands: &[],
            build: [100.0, 100.0, 100.0],
            mode: ViewMode::Solid,
            slice: None,
            current_layer: 0,
            cut: None,
            show_plate: false,
        };
        let view = View::new(&Camera::default(), 64.0, 64.0);
        renderer.draw(
            &device,
            &queue,
            &scene,
            &view,
            [64, 64],
            SceneRevision::default(),
        );
        let pixels = renderer.read_rgba(&device, &queue).unwrap();
        assert_eq!(pixels.len(), 64 * 64 * 4);
        assert_eq!(&pixels[..3], &BG_TOP);
        let last = pixels.len() - 4;
        assert!((pixels[last] as i32 - BG_BOTTOM[0] as i32).abs() <= 1);
        assert!((pixels[last + 1] as i32 - BG_BOTTOM[1] as i32).abs() <= 1);
        assert!((pixels[last + 2] as i32 - BG_BOTTOM[2] as i32).abs() <= 1);
    }

    #[test]
    fn meshes_render_and_upload_only_when_their_revision_changes() {
        let Some((device, queue)) = headless_device() else {
            eprintln!("no Vulkan adapter for the headless GPU test");
            return;
        };
        let mut renderer = GpuRenderer::new(&device);
        let models = vec![crate::mesh::sample_pawn()];
        let scene = Scene {
            models: &models,
            selection: &[],
            support_mesh: None,
            cut_cap: None,
            overhang: None,
            islands: &[],
            build: [218.88, 122.88, 200.0],
            mode: ViewMode::Solid,
            slice: None,
            current_layer: 0,
            cut: None,
            show_plate: false,
        };
        let view = View::new(&Camera::default(), 400.0, 300.0);
        renderer.draw(
            &device,
            &queue,
            &scene,
            &view,
            [400, 300],
            SceneRevision::default(),
        );
        let pixels = renderer.read_rgba(&device, &queue).unwrap();
        let warm = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] > 120 && p[2] < p[0])
            .count();
        assert!(warm > 100, "the pawn should cover some pixels: {warm}");
        assert_eq!(renderer.upload_count, 1);
        renderer.draw(
            &device,
            &queue,
            &scene,
            &view,
            [400, 300],
            SceneRevision::default(),
        );
        assert_eq!(renderer.upload_count, 1, "an idle redraw uploaded again");
        renderer.draw(
            &device,
            &queue,
            &scene,
            &view,
            [400, 300],
            SceneRevision {
                models: 1,
                derived: 0,
            },
        );
        assert_eq!(renderer.upload_count, 2);
    }

    #[test]
    fn picked_pieces_are_tinted() {
        let Some((device, queue)) = headless_device() else {
            eprintln!("no Vulkan adapter for the headless GPU test");
            return;
        };
        let mut renderer = GpuRenderer::new(&device);
        let models = vec![crate::mesh::sample_pawn()];
        let mut scene = Scene {
            models: &models,
            selection: &[],
            support_mesh: None,
            cut_cap: None,
            overhang: None,
            islands: &[],
            build: [218.88, 122.88, 200.0],
            mode: ViewMode::Solid,
            slice: None,
            current_layer: 0,
            cut: None,
            show_plate: false,
        };
        let view = View::new(&Camera::default(), 400.0, 300.0);
        renderer.draw(
            &device,
            &queue,
            &scene,
            &view,
            [400, 300],
            SceneRevision::default(),
        );
        let plain = renderer.read_rgba(&device, &queue).unwrap();
        let blue = |pixels: &[u8]| {
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|p| p[2] > p[0] + 20)
                .count()
        };
        let before = blue(&plain);
        scene.selection = &[0];
        renderer.draw(
            &device,
            &queue,
            &scene,
            &view,
            [400, 300],
            SceneRevision::default(),
        );
        let picked = renderer.read_rgba(&device, &queue).unwrap();
        assert!(
            blue(&picked) > before + 100,
            "picked pawn should turn blue: {before} -> {}",
            blue(&picked)
        );
    }

    #[test]
    fn plate_and_shadow_geometry_render_on_the_gpu() {
        let Some((device, queue)) = headless_device() else {
            eprintln!("no Vulkan adapter for the headless GPU test");
            return;
        };
        let mut renderer = GpuRenderer::new(&device);
        let models = vec![crate::mesh::sample_pawn()];
        let scene = Scene {
            models: &models,
            selection: &[],
            support_mesh: None,
            cut_cap: None,
            overhang: None,
            islands: &[],
            build: [218.88, 122.88, 200.0],
            mode: ViewMode::Solid,
            slice: None,
            current_layer: 0,
            cut: None,
            show_plate: true,
        };
        let view = View::new(&Camera::default(), 400.0, 300.0);
        renderer.draw(
            &device,
            &queue,
            &scene,
            &view,
            [400, 300],
            SceneRevision::default(),
        );
        let pixels = renderer.read_rgba(&device, &queue).unwrap();
        let plate = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[..3] == [36, 40, 48])
            .count();
        assert!(plate > 100, "the plate is missing: {plate}");
        let border = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[..3] == [86, 133, 196])
            .count();
        assert!(border > 0, "plate lines are missing");
        let warm = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] > 120 && p[2] < p[0])
            .count();
        assert!(warm > 100);
    }

    #[test]
    fn a_drawn_trunk_and_foot_stay_wider_at_the_base_close_up() {
        let Some((device, queue)) = headless_device() else {
            eprintln!("no Vulkan adapter for the headless GPU test");
            return;
        };
        let mut renderer = GpuRenderer::new(&device);
        let foot = crate::supports::Foot {
            id: 0,
            pos: Vec3::new(0.0, 0.0, 0.0),
            diameter: 3.4,
            trunk_id: 0,
            model_id: 0,
        };
        let mut mesh = crate::supports::meshgen::foot(&foot);
        crate::supports::meshgen::append(
            &mut mesh,
            &crate::supports::meshgen::frustum(
                Vec3::new(0.0, 0.0, 0.8),
                0.8,
                Vec3::new(0.0, 0.0, 20.0),
                0.8,
                12,
            ),
        );
        let scene = Scene {
            models: &[],
            selection: &[],
            support_mesh: Some(&mesh),
            cut_cap: None,
            overhang: None,
            islands: &[],
            build: [218.88, 122.88, 200.0],
            mode: ViewMode::Solid,
            slice: None,
            current_layer: 0,
            cut: None,
            show_plate: false,
        };
        let camera = Camera {
            pitch: 0.05,
            target: Vec3::new(0.0, 0.0, 6.0),
            dist: 55.0,
            ..Default::default()
        };
        let view = View::new(&camera, 400.0, 400.0);
        renderer.draw(
            &device,
            &queue,
            &scene,
            &view,
            [400, 400],
            SceneRevision::default(),
        );
        let pixels = renderer.read_rgba(&device, &queue).unwrap();
        let mut widths = Vec::new();
        for row in pixels.as_chunks::<4>().0.as_chunks::<400>().0.iter() {
            let mut first = None;
            let mut last = 0;
            for (x, p) in row.iter().enumerate() {
                if p[0] > 50 && p[2] > p[0] {
                    first.get_or_insert(x);
                    last = x;
                }
            }
            if let Some(first) = first {
                widths.push(last - first + 1);
            }
        }
        assert!(
            widths.len() > 40,
            "the support is missing: {} rows",
            widths.len()
        );
        let foot_row = widths[widths.len() - 3];
        let middle = widths[widths.len() / 2];
        assert!(
            foot_row > middle + 4,
            "the foot should be wider: {foot_row} vs {middle}"
        );
    }

    #[test]
    fn sliced_lines_use_the_current_layer_and_cache() {
        let Some((device, queue)) = headless_device() else {
            eprintln!("no Vulkan adapter for the headless GPU test");
            return;
        };
        let mut renderer = GpuRenderer::new(&device);
        let pawn = crate::mesh::sample_pawn();
        let slice = crate::slicer::slice(std::slice::from_ref(&pawn), 0.2);
        let current_layer = slice.layers.len() / 3;
        let scene = Scene {
            models: &[],
            selection: &[],
            support_mesh: None,
            cut_cap: None,
            overhang: None,
            islands: &[],
            build: [218.88, 122.88, 200.0],
            mode: ViewMode::Sliced,
            slice: Some(&slice),
            current_layer,
            cut: None,
            show_plate: false,
        };
        let view = View::new(&Camera::default(), 400.0, 300.0);
        renderer.draw(
            &device,
            &queue,
            &scene,
            &view,
            [400, 300],
            SceneRevision::default(),
        );
        let pixels = renderer.read_rgba(&device, &queue).unwrap();
        let current = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[..3] == [233, 86, 74])
            .count();
        assert!(current > 0, "the current layer was not drawn");
        assert_eq!(renderer.slice_rebuilds, 1);
        renderer.draw(
            &device,
            &queue,
            &scene,
            &view,
            [400, 300],
            SceneRevision::default(),
        );
        assert_eq!(renderer.slice_rebuilds, 1, "slice lines uploaded again");
    }

    #[test]
    fn support_meshes_are_shaded_on_the_gpu() {
        let Some((device, queue)) = headless_device() else {
            eprintln!("no Vulkan adapter for the headless GPU test");
            return;
        };
        let mut renderer = GpuRenderer::new(&device);
        let support = crate::mesh::sample_pawn();
        let scene = Scene {
            models: &[],
            selection: &[],
            support_mesh: Some(&support),
            cut_cap: None,
            overhang: None,
            islands: &[],
            build: [218.88, 122.88, 200.0],
            mode: ViewMode::Solid,
            slice: None,
            current_layer: 0,
            cut: None,
            show_plate: false,
        };
        let view = View::new(&Camera::default(), 400.0, 300.0);
        renderer.draw(
            &device,
            &queue,
            &scene,
            &view,
            [400, 300],
            SceneRevision::default(),
        );
        let pixels = renderer.read_rgba(&device, &queue).unwrap();
        let support_pixels = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] > 60 && p[2] > p[0])
            .count();
        assert!(
            support_pixels > 100,
            "support was not drawn: {support_pixels}"
        );
        assert_eq!(renderer.upload_count, 1);
    }
}
