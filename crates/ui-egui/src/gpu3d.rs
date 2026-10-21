//! 3D point cloud renderer: a perspective camera over the session's clouds.
//!
//! Unlike the 2D canvas, the geometry here is world-space 3D and does not change when the camera
//! moves, so the vertex buffer is uploaded once per level/colour change and each frame only the
//! 80-byte view-projection uniform is written. Points are drawn as 6-vertex screen-space squares
//! (instanced, one instance per point): a `PointList` would need `gl_PointSize`, which WebGPU's
//! WGSL does not have.
//!
//! The pipeline lives in egui's callback resources, next to the 2D canvas pipelines, and shares
//! its target format and stencil attachment.

use std::sync::{Arc, Mutex};

use egui_wgpu::wgpu;
use egui_wgpu::wgpu::util::DeviceExt;

use crate::gpu::GpuTarget;

/// `[x, y, z]` f32 + RGBA8: 16 bytes per point.
pub const VERTEX3D_SIZE: usize = 16;
const UNIFORM3D_SIZE: u64 = 80;
/// Points per drawn frame. A 1.5M-point upload is 24 MB; the LOD rarely picks that many.
pub const MAX_SCENE_POINTS: usize = 1_500_000;

/// One point as the GPU sees it: world position and premultiplied-ready colour.
#[derive(Clone, Copy, Debug)]
pub struct Point3d {
    pub pos: [f32; 3],
    pub color: [u8; 4],
}

/// The points of the current level, built on the UI thread and uploaded by [`Cloud3dCallback`].
#[derive(Clone, Debug, Default)]
pub struct Scene3d {
    pub points: Vec<Point3d>,
}

pub type SceneSlot = Arc<Mutex<Option<Scene3d>>>;

/// Per-frame inputs for the paint callback.
pub struct Cloud3dCallback {
    /// Identifies the scene in `slot`; the GPU buffer is rebuilt when it changes.
    pub key: u64,
    pub slot: SceneSlot,
    /// Column-major world → clip matrix.
    pub view_proj: [f32; 16],
    /// Canvas size in points (the callback rect).
    pub viewport: [f32; 2],
    /// Point square side in points.
    pub point_size: f32,
    /// The render target's stencil format, so the pipeline matches the pass (depth stays off).
    pub stencil: Option<wgpu::TextureFormat>,
}

struct Resources3d {
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    buffer: Option<wgpu::Buffer>,
    count: u32,
    key: Option<u64>,
    linear_out: bool,
}

/// Create the pipeline in the renderer's callback resources. Call once, next to
/// [`crate::gpu::install`].
pub fn install(rs: &egui_wgpu::RenderState, target: GpuTarget) {
    let res = Resources3d::new(&rs.device, target);
    rs.renderer.write().callback_resources.insert(res);
}

impl Resources3d {
    fn new(device: &wgpu::Device, target: GpuTarget) -> Self {
        let module =
            device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("cad_cloud3d"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cad_cloud3d"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cad_cloud3d"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cad_cloud3d_view"),
            size: UNIFORM3D_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cad_cloud3d"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
        });
        const ATTRS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4];
        let instance_layout =
            wgpu::VertexBufferLayout { array_stride: VERTEX3D_SIZE as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &ATTRS };
        // The pass carries a stencil attachment when the 2D canvas asked for one; a pipeline in
        // that pass must name its format, but clouds never test or write it.
        let depth_stencil = target.stencil.map(|format| wgpu::DepthStencilState {
            format,
            depth_write_enabled: None,
            depth_compare: None,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cad_cloud3d"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                buffers: &[Some(instance_layout)],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
            depth_stencil,
            multisample: wgpu::MultisampleState { count: 1, mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target.format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        Self { pipeline, uniform, bind_group, buffer: None, count: 0, key: None, linear_out: target.format.is_srgb() }
    }
}

impl egui_wgpu::CallbackTrait for Cloud3dCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(res) = resources.get_mut::<Resources3d>() else { return Vec::new() };
        if res.key != Some(self.key) {
            let scene = self.slot.lock().ok().and_then(|mut s| s.take());
            if let Some(scene) = scene {
                if scene.points.is_empty() {
                    res.buffer = None;
                    res.count = 0;
                } else {
                    let mut bytes = Vec::with_capacity(scene.points.len() * VERTEX3D_SIZE);
                    for p in &scene.points {
                        for v in p.pos {
                            bytes.extend_from_slice(&v.to_le_bytes());
                        }
                        bytes.extend_from_slice(&p.color);
                    }
                    res.buffer = Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("cad_cloud3d_points"),
                        contents: &bytes,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    }));
                    res.count = scene.points.len() as u32;
                }
                res.key = Some(self.key);
            }
        }
        let ppp = screen.pixels_per_point;
        let linear = if res.linear_out { 1.0f32 } else { 0.0 };
        let mut bytes = Vec::with_capacity(UNIFORM3D_SIZE as usize);
        for v in self.view_proj {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        for v in [self.point_size * ppp, self.viewport[0] * ppp, self.viewport[1] * ppp, linear] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        queue.write_buffer(&res.uniform, 0, &bytes);
        Vec::new()
    }

    fn paint(&self, info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &egui_wgpu::CallbackResources) {
        let Some(res) = resources.get::<Resources3d>() else { return };
        let Some(buffer) = &res.buffer else { return };
        if res.count == 0 || res.key != Some(self.key) {
            return;
        }
        let [sw, sh] = info.screen_size_px;
        if sw == 0 || sh == 0 {
            return;
        }
        pass.set_viewport(0.0, 0.0, sw as f32, sh as f32, 0.0, 1.0);
        pass.set_pipeline(&res.pipeline);
        pass.set_bind_group(0, &res.bind_group, &[]);
        pass.set_vertex_buffer(0, buffer.slice(..));
        // Six vertices per instance: two triangles make the square.
        pass.draw(0..6, 0..res.count);
    }
}

const SHADER: &str = r#"
struct View {
    view_proj: mat4x4<f32>,
    // x: point size in device pixels, y/z: viewport in device pixels, w: linear target
    params: vec4<f32>,
};
@group(0) @binding(0) var<uniform> view: View;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

// `c` is premultiplied sRGB; a linear target gets it linearised, still premultiplied.
fn out_color(c: vec4<f32>) -> vec4<f32> {
    if (view.params.w > 0.5) {
        let s = c.rgb / max(c.a, 0.0001);
        let lo = s / 12.92;
        let hi = pow((s + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
        return vec4<f32>(select(hi, lo, s <= vec3<f32>(0.04045)) * c.a, c.a);
    }
    return c;
}

@vertex
fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) p: vec3<f32>,
    @location(1) c: vec4<f32>,
) -> VOut {
    var clip = view.view_proj * vec4<f32>(p, 1.0);
    // Two counter-clockwise triangles in the 2x2 screen square:
    // 0,1,2 = (-1,-1), (1,-1), (-1,1) and 3,4,5 = (-1,-1), (-1,1), (1,1).
    let second = vi >= 3u;
    let k = select(vi, vi - 3u, second);
    let right = select(k == 1u, k == 2u, second);
    let up_corner = select(k == 2u, k == 1u, second);
    let o = vec2<f32>(select(-1.0, 1.0, right), select(-1.0, 1.0, up_corner));
    let half = view.params.x * 0.5;
    let ndc = vec2<f32>(half / view.params.y, half / view.params.z);
    clip.x += o.x * ndc.x * clip.w;
    clip.y += o.y * ndc.y * clip.w;
    var out: VOut;
    out.pos = clip;
    out.color = out_color(c);
    return out;
}

@fragment
fn fs(v: VOut) -> @location(0) vec4<f32> {
    return v.color;
}
"#;
