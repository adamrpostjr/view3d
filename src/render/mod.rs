//! wgpu resources and the egui paint callback that draws the 3D scene.

use eframe::egui_wgpu::{self, wgpu};
use glam::Mat4;
use wgpu::util::DeviceExt as _;

use crate::mesh::{Mesh, Vertex};
use crate::settings::DrawMode;

/// One 256-byte aligned slot per line draw (model axes, HUD flower, 3 labels).
const LINE_SLOTS: u64 = 5;
const LINE_SLOT_SIZE: u64 = 256;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    mvp: [[f32; 4]; 4],
    ambient: [f32; 4],
    directive: [f32; 4],
    light_dir: [f32; 4],
    flags: [f32; 4],
}

/// Everything the callback needs for one frame.
#[derive(Clone)]
pub struct SceneParams {
    pub mvp: Mat4,
    pub ambient: [f32; 4],
    pub directive: [f32; 4],
    pub light_dir: [f32; 3],
    pub zoom_inv: f32,
    pub has_colors: bool,
    pub draw_mode: DrawMode,
    pub draw_axes: bool,
    /// Model axes, HUD flower, then the X/Y/Z label transforms.
    pub line_mvps: [Mat4; LINE_SLOTS as usize],
}

struct GpuMesh {
    verts: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    edges: Option<(wgpu::Buffer, u32)>,
}

pub struct Scene {
    backdrop: wgpu::RenderPipeline,
    /// Filled modes, indexed by `DrawMode::fill_index`.
    filled: Vec<wgpu::RenderPipeline>,
    wireframe: wgpu::RenderPipeline,
    line: wgpu::RenderPipeline,
    line_hud: wgpu::RenderPipeline,

    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    line_uniforms: wgpu::Buffer,
    line_bind_group: wgpu::BindGroup,

    /// Flower axes + X/Y/Z letters, in that order.
    hud_lines: wgpu::Buffer,
    /// Model-space axes, resized to the mesh bounds on load.
    axis_lines: wgpu::Buffer,

    mesh: Option<GpuMesh>,
}

const VERTEX_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: std::mem::size_of::<Vertex>() as u64,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &[
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Unorm8x4,
            offset: 12,
            shader_location: 1,
        },
    ],
};

fn depth_state(write: bool, test: bool) -> Option<wgpu::DepthStencilState> {
    Some(wgpu::DepthStencilState {
        format: wgpu::TextureFormat::Depth32Float,
        depth_write_enabled: Some(write),
        depth_compare: Some(if test {
            wgpu::CompareFunction::Less
        } else {
            wgpu::CompareFunction::Always
        }),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    })
}

impl Scene {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let scene_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("view3d scene"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/scene.wgsl").into()),
        });
        let line_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("view3d lines"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/lines.wgsl").into()),
        });

        // A single uniform block per bind group; the line pipelines use a
        // dynamic offset so one buffer can hold every line draw's matrix.
        let uniform_entry = |dynamic: bool, min: u64| wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: std::num::NonZeroU64::new(min),
            },
            count: None,
        };

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("view3d uniforms"),
            entries: &[uniform_entry(false, std::mem::size_of::<Uniforms>() as u64)],
        });
        let line_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("view3d line uniforms"),
            entries: &[uniform_entry(true, 64)],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("view3d scene layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let line_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("view3d line layout"),
            bind_group_layouts: &[Some(&line_bgl)],
            immediate_size: 0,
        });

        let make_pipeline = |label: &str,
                             shader: &wgpu::ShaderModule,
                             pipeline_layout: &wgpu::PipelineLayout,
                             vs: &str,
                             fs: &str,
                             buffers: &[Option<wgpu::VertexBufferLayout<'static>>],
                             topology: wgpu::PrimitiveTopology,
                             depth: Option<wgpu::DepthStencilState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(pipeline_layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some(vs),
                    buffers,
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(fs),
                    targets: &[Some(target_format.into())],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology,
                    ..Default::default()
                },
                depth_stencil: depth,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };

        let backdrop = make_pipeline(
            "backdrop",
            &scene_shader,
            &layout,
            "vs_backdrop",
            "fs_backdrop",
            &[],
            wgpu::PrimitiveTopology::TriangleStrip,
            depth_state(true, false),
        );

        let filled = DrawMode::FILLED
            .iter()
            .map(|mode| {
                make_pipeline(
                    mode.label(),
                    &scene_shader,
                    &layout,
                    "vs_mesh",
                    mode.entry_point(),
                    &[Some(VERTEX_LAYOUT)],
                    wgpu::PrimitiveTopology::TriangleList,
                    depth_state(true, true),
                )
            })
            .collect();

        let wireframe = make_pipeline(
            "wireframe",
            &scene_shader,
            &layout,
            "vs_mesh",
            "fs_wireframe",
            &[Some(VERTEX_LAYOUT)],
            wgpu::PrimitiveTopology::LineList,
            depth_state(true, true),
        );

        let line = make_pipeline(
            "axes",
            &line_shader,
            &line_layout,
            "vs_line",
            "fs_line",
            &[Some(VERTEX_LAYOUT)],
            wgpu::PrimitiveTopology::LineList,
            depth_state(true, true),
        );
        // The HUD flower must sit on top of everything, so it ignores depth.
        let line_hud = make_pipeline(
            "axis flower",
            &line_shader,
            &line_layout,
            "vs_line",
            "fs_line",
            &[Some(VERTEX_LAYOUT)],
            wgpu::PrimitiveTopology::LineList,
            depth_state(false, false),
        );

        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view3d uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("view3d uniforms"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });

        let line_uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view3d line uniforms"),
            size: LINE_SLOTS * LINE_SLOT_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let line_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("view3d line uniforms"),
            layout: &line_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &line_uniforms,
                    offset: 0,
                    size: std::num::NonZeroU64::new(64),
                }),
            }],
        });

        let hud_lines = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("view3d hud lines"),
            contents: bytemuck::cast_slice(&hud_line_verts()),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let axis_lines = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view3d model axes"),
            size: (6 * std::mem::size_of::<Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            backdrop,
            filled,
            wireframe,
            line,
            line_hud,
            uniforms,
            bind_group,
            line_uniforms,
            line_bind_group,
            hud_lines,
            axis_lines,
            mesh: None,
        }
    }

    pub fn upload_mesh(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, mesh: &Mesh) {
        let verts = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("view3d mesh vertices"),
            contents: bytemuck::cast_slice(&mesh.verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("view3d mesh indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        self.mesh = Some(GpuMesh {
            verts,
            indices,
            index_count: mesh.indices.len() as u32,
            edges: None,
        });

        // Model-space axes, extended past the model like fstl's Axis::setScale.
        let (min, max) = (mesh.bounds.min, mesh.bounds.max);
        let margin = 0.25 * mesh.bounds.size().max_element();
        let mut verts = [Vertex::default(); 6];
        for axis in 0..3 {
            let color = 0xff00_0000 | 0xffu32 << (8 * axis);
            let (mut a, mut b) = ([0.0f32; 3], [0.0f32; 3]);
            a[axis] = min[axis] - margin;
            b[axis] = max[axis] + margin;
            verts[axis * 2] = Vertex { pos: a, color };
            verts[axis * 2 + 1] = Vertex { pos: b, color };
        }
        queue.write_buffer(&self.axis_lines, 0, bytemuck::cast_slice(&verts));
    }

    /// Builds the wireframe edge buffer the first time it is needed.
    pub fn ensure_edges(&mut self, device: &wgpu::Device, mesh: &Mesh) {
        let Some(gpu) = self.mesh.as_mut() else {
            return;
        };
        if gpu.edges.is_some() {
            return;
        }
        let edges = mesh.edge_indices();
        let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("view3d mesh edges"),
            contents: bytemuck::cast_slice(&edges),
            usage: wgpu::BufferUsages::INDEX,
        });
        gpu.edges = Some((buf, edges.len() as u32));
    }

    fn prepare(&self, queue: &wgpu::Queue, p: &SceneParams) {
        let u = Uniforms {
            mvp: p.mvp.to_cols_array_2d(),
            ambient: p.ambient,
            directive: p.directive,
            light_dir: [p.light_dir[0], p.light_dir[1], p.light_dir[2], p.zoom_inv],
            flags: [if p.has_colors { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
        };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&u));

        for (i, m) in p.line_mvps.iter().enumerate() {
            queue.write_buffer(
                &self.line_uniforms,
                i as u64 * LINE_SLOT_SIZE,
                bytemuck::cast_slice(&m.to_cols_array()),
            );
        }
    }

    fn paint(&self, rp: &mut wgpu::RenderPass<'static>, p: &SceneParams) {
        rp.set_pipeline(&self.backdrop);
        rp.set_bind_group(0, &self.bind_group, &[]);
        rp.draw(0..4, 0..1);

        if let Some(gpu) = &self.mesh {
            rp.set_vertex_buffer(0, gpu.verts.slice(..));
            match (p.draw_mode, &gpu.edges) {
                (DrawMode::Wireframe, Some((edges, count))) => {
                    rp.set_pipeline(&self.wireframe);
                    rp.set_index_buffer(edges.slice(..), wgpu::IndexFormat::Uint32);
                    rp.draw_indexed(0..*count, 0, 0..1);
                }
                (mode, _) => {
                    let idx = mode.fill_index().unwrap_or(0);
                    rp.set_pipeline(&self.filled[idx]);
                    rp.set_index_buffer(gpu.indices.slice(..), wgpu::IndexFormat::Uint32);
                    rp.draw_indexed(0..gpu.index_count, 0, 0..1);
                }
            }
        }

        if p.draw_axes {
            // Model-space axes, then the HUD flower and its X/Y/Z labels.
            rp.set_pipeline(&self.line);
            rp.set_bind_group(0, &self.line_bind_group, &[0]);
            rp.set_vertex_buffer(0, self.axis_lines.slice(..));
            rp.draw(0..6, 0..1);

            rp.set_pipeline(&self.line_hud);
            rp.set_vertex_buffer(0, self.hud_lines.slice(..));
            for (slot, range) in [(1u32, 0..6u32), (2, 6..10), (3, 10..16), (4, 16..22)] {
                rp.set_bind_group(0, &self.line_bind_group, &[slot * LINE_SLOT_SIZE as u32]);
                rp.draw(range, 0..1);
            }
        }
    }
}

/// Unit axis lines plus the little X/Y/Z letters, matching fstl's `axis.cpp`.
fn hud_line_verts() -> Vec<Vertex> {
    const X_LET: [f32; 12] = [
        -0.1, -0.2, 0.0, 0.1, 0.2, 0.0, 0.1, -0.2, 0.0, -0.1, 0.2, 0.0,
    ];
    const Y_LET: [f32; 18] = [
        0.0, -0.2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.1, 0.2, 0.0, 0.0, 0.0, 0.0, -0.1, 0.2, 0.0,
    ];
    const Z_LET: [f32; 18] = [
        -0.1, -0.2, 0.0, 0.1, -0.2, 0.0, 0.1, -0.2, 0.0, -0.1, 0.2, 0.0, -0.1, 0.2, 0.0, 0.1, 0.2,
        0.0,
    ];

    let mut out = Vec::with_capacity(22);
    for axis in 0..3usize {
        let color = 0xff00_0000 | 0xffu32 << (8 * axis);
        let mut end = [0.0f32; 3];
        end[axis] = 1.0;
        out.push(Vertex {
            pos: [0.0; 3],
            color,
        });
        out.push(Vertex { pos: end, color });
    }
    for (axis, letter) in [&X_LET[..], &Y_LET[..], &Z_LET[..]].iter().enumerate() {
        let color = 0xff00_0000 | 0xffu32 << (8 * axis);
        for p in letter.chunks_exact(3) {
            out.push(Vertex {
                pos: [p[0], p[1], p[2]],
                color,
            });
        }
    }
    out
}

pub struct SceneCallback {
    pub params: SceneParams,
}

impl egui_wgpu::CallbackTrait for SceneCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(scene) = resources.get::<Scene>() {
            scene.prepare(queue, &self.params);
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        rp: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(scene) = resources.get::<Scene>() else {
            return;
        };
        // egui has already set the viewport and scissor to our rect.
        scene.paint(rp, &self.params);
    }
}
