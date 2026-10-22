//! 3D point cloud renderer: a perspective camera over the session's clouds.
//!
//! Unlike the 2D canvas, the geometry here is world-space 3D and does not change when the camera
//! moves, so the vertex buffer is uploaded once per level/colour change and each frame only the
//! 80-byte view-projection uniform is written. Points are drawn as 6-vertex screen-space squares
//! (instanced, one instance per point): a `PointList` would need `gl_PointSize`, which WebGPU's
//! WGSL does not have.
//!
//! The pipeline lives in egui's callback resources, next to the 2D canvas pipelines, and shares
//! its target format and stencil attachment. It has **no depth buffer**: the 2D canvas owns the
//! pass, and adding a depth attachment would change the upstream pipeline. Two things take over
//! that job:
//!
//! - the **front wall cut**: the points between the eye and a given distance are dropped, so the
//!   camera can look into a room without cropping the walls out of the cloud;
//! - the **depth cue**: the further a point is behind the orbit target, the more it fades into the
//!   background and the smaller its square gets, so the near geometry stands out from the back
//!   wall that would otherwise be mixed into it.

use std::sync::{Arc, Mutex};

use egui_wgpu::wgpu;
use egui_wgpu::wgpu::util::DeviceExt;

use crate::gpu::GpuTarget;

/// `[x, y, z]` f32 + RGBA8: 16 bytes per point.
pub const VERTEX3D_SIZE: usize = 16;
/// Points in the uploaded scene. Only the part of the scan near the orbit target is uploaded, so
/// this is the visible budget, not the cloud size: 3M points is 48 MB and still a small vertex load
/// for a frame, and at deep zoom the full-resolution level fits inside it.
pub const MAX_SCENE_POINTS: usize = 3_000_000;

/// The per-frame uniform of the 3D pipeline. The layout is `repr(C)` with nothing but `f32`
/// arrays, so the struct is cast into the buffer as it is (`bytemuck`).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct View3dUniform {
    /// Column-major world → clip matrix.
    pub view_proj: [f32; 16],
    /// x: point size in device pixels, y/z: viewport in device pixels, w: linear target.
    pub params: [f32; 4],
    /// x: the near cut in metres (0 = off), y: the depth cue's span in metres, z: the depth of
    /// the orbit target, w: the cue's strength (0..1).
    pub clip: [f32; 4],
    /// xyz: the eye, w: unused.
    pub eye: [f32; 4],
    /// xyz: the unit view direction, w: unused.
    pub forward: [f32; 4],
    /// The background colour (sRGB), the far end of the depth cue.
    pub bg: [f32; 4],
    /// x: the cloud's opacity, y: whether the x-ray mask is on, z: the opacity as a byte (the
    /// mask's threshold), w: unused.
    pub misc: [f32; 4],
}

const UNIFORM3D_SIZE: u64 = std::mem::size_of::<View3dUniform>() as u64;

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
    /// The eye, for the depth of every point.
    pub eye: [f32; 3],
    /// The unit view direction, for the depth of every point.
    pub forward: [f32; 3],
    /// The points closer to the eye than this are dropped (the front wall cut). 0 = off.
    pub near_cut: f32,
    /// How far behind the target the depth cue reaches its full strength, in metres.
    pub cue_span: f32,
    /// The depth of the orbit target, where the depth cue starts.
    pub target_depth: f32,
    /// How strong the depth cue is (0..1); 1 fades the farthest points by 45%.
    pub cue: f32,
    /// The background colour (sRGB), the far end of the depth cue.
    pub bg: [f32; 4],
    /// The cloud's opacity (0..1) and whether the x-ray mask is on.
    pub opacity: f32,
    /// Thin the points by a deterministic mask on the instance index, so a wall seen edge-on does
    /// not become a solid slab of pixels when the cloud is semi-transparent.
    pub xray: bool,
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
        let uniform = View3dUniform {
            view_proj: self.view_proj,
            params: [self.point_size * ppp, self.viewport[0] * ppp, self.viewport[1] * ppp, linear],
            clip: [self.near_cut, self.cue_span, self.target_depth, self.cue],
            eye: [self.eye[0], self.eye[1], self.eye[2], 0.0],
            forward: [self.forward[0], self.forward[1], self.forward[2], 0.0],
            bg: self.bg,
            misc: [self.opacity, if self.xray { 1.0 } else { 0.0 }, (self.opacity.clamp(0.0, 1.0) * 255.0).round(), 0.0],
        };
        queue.write_buffer(&res.uniform, 0, bytemuck::bytes_of(&uniform));
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
    // x: near cut in metres (0 = off), y: the depth cue's span, z: the target's depth,
    // w: the cue's strength (0..1)
    clip: vec4<f32>,
    // xyz: the eye, w: unused
    eye: vec4<f32>,
    // xyz: the unit view direction, w: unused
    forward: vec4<f32>,
    // the background colour (sRGB), the far end of the depth cue
    bg: vec4<f32>,
    // x: the cloud's opacity, y: the x-ray mask on, z: the opacity as a byte (the mask's
    // threshold), w: unused
    misc: vec4<f32>,
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
    @builtin(instance_index) ii: u32,
    @location(0) p: vec3<f32>,
    @location(1) c: vec4<f32>,
) -> VOut {
    var clip = view.view_proj * vec4<f32>(p, 1.0);
    // The depth of the point along the view direction: the distance from the eye. `clip.w` would
    // do it in perspective, but the orthographic projection leaves it at 1.
    let depth = dot(p - view.eye.xyz, view.forward.xyz);
    var out: VOut;
    // The x-ray: with a semi-transparent cloud the points are thinned by a deterministic mask on
    // the instance index, so a wall seen edge-on does not turn into a solid slab of pixels. Without
    // a depth buffer this is what keeps the drawing under the cloud readable.
    let opacity = clamp(view.misc.x, 0.0, 1.0);
    if (view.misc.y > 0.5 && opacity < 0.999) {
        let h = (ii * 2654435761u >> 16) & 0xFFu;
        if (h >= u32(view.misc.z)) {
            out.pos = vec4<f32>(0.0, 0.0, 2.0, 1.0);
            out.color = vec4<f32>(0.0);
            return out;
        }
    }
    // The front wall cut: the points between the eye and the cut are dropped, so the camera can
    // look into a room without cropping the walls out of the cloud. The vertex goes outside the
    // clip volume, so nothing is drawn for it.
    if (view.clip.x > 0.0 && depth < view.clip.x) {
        out.pos = vec4<f32>(0.0, 0.0, 2.0, 1.0);
        out.color = vec4<f32>(0.0);
        return out;
    }
    // The depth cue: the further the point is behind the orbit target, the more it fades into the
    // background (up to 45%) and the smaller its square gets. The ramp is quadratic, so the
    // points just behind the target keep their colour and only the far ones recede.
    let span = max(view.clip.y, 0.0001);
    let t = clamp((depth - view.clip.z) / span, 0.0, 1.0);
    let far = t * t;
    let cue = view.clip.w * far;
    // The colour is premultiplied: the opacity scales the colour and its alpha together.
    var col = vec4<f32>(mix(c.rgb, view.bg.rgb, cue * 0.45) * opacity, c.a * opacity);
    // Two counter-clockwise triangles in the 2x2 screen square:
    // 0,1,2 = (-1,-1), (1,-1), (-1,1) and 3,4,5 = (-1,-1), (-1,1), (1,1).
    let second = vi >= 3u;
    let k = select(vi, vi - 3u, second);
    let right = select(k == 1u, k == 2u, second);
    let up_corner = select(k == 2u, k == 1u, second);
    let o = vec2<f32>(select(-1.0, 1.0, right), select(-1.0, 1.0, up_corner));
    // The square is drawn at a whole-pixel size, rounding the size the level asks for and never
    // going below one pixel, and its centre is snapped to a half pixel. A fractional square at a
    // fractional position covers a different set of pixels for every sub-pixel of camera motion,
    // which made the cloud crawl while orbiting. The size does not shrink with depth: the depth cue
    // only fades the colour, because a smaller far square left one-pixel holes in the fitted view.
    // The old half-extent was half of the square asked for, so the points were sub-pixel and dim.
    let side = max(1.0, floor(view.params.x + 0.5));
    let half_viewport = vec2<f32>(view.params.y * 0.5, view.params.z * 0.5);
    if (clip.w > 0.0) {
        // The viewport is the full size in device pixels, so half of it spans the [-1, 1] NDC range.
        let centre = clip.xy / clip.w * half_viewport;
        let snapped = floor(centre) + vec2<f32>(0.5);
        clip.x = snapped.x / half_viewport.x * clip.w;
        clip.y = snapped.y / half_viewport.y * clip.w;
    }
    // A square of `side` device pixels needs an NDC half-extent of `side / params.y` on x (and
    // `side / params.z` on y), times `clip.w` to get back into clip space.
    let extent = vec2<f32>(side / view.params.y, side / view.params.z);
    clip.x += o.x * extent.x * clip.w;
    clip.y += o.y * extent.y * clip.w;
    out.pos = clip;
    out.color = out_color(col);
    return out;
}

@fragment
fn fs(v: VOut) -> @location(0) vec4<f32> {
    return v.color;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// The uniform is cast into the buffer as it is, so the layout must have no padding and every
    /// member must sit where WGSL expects it (16-byte aligned, 144 bytes in all).
    #[test]
    fn the_uniform_layout_is_ready_for_the_gpu() {
        assert_eq!(std::mem::size_of::<View3dUniform>(), 160);
        assert_eq!(UNIFORM3D_SIZE % 16, 0);
        let u = View3dUniform {
            view_proj: [0.0; 16],
            params: [1.0, 2.0, 3.0, 4.0],
            clip: [5.0, 6.0, 7.0, 8.0],
            eye: [9.0, 10.0, 11.0, 0.0],
            forward: [12.0, 13.0, 14.0, 0.0],
            bg: [0.25, 0.5, 0.75, 1.0],
            misc: [0.35, 1.0, 0.0, 0.0],
        };
        let bytes = bytemuck::bytes_of(&u);
        let at = |offset: usize| f32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]]);
        assert_eq!(at(64), 1.0, "params");
        assert_eq!(at(80), 5.0, "clip");
        assert_eq!(at(96), 9.0, "eye");
        assert_eq!(at(112), 12.0, "forward");
        assert_eq!(at(128), 0.25, "bg");
        assert_eq!(at(144), 0.35, "misc");
        for offset in [0, 64, 80, 96, 112, 128, 144] {
            assert_eq!(offset % 16, 0, "every member is 16-byte aligned");
        }
    }

    /// The same WGSL the GPU runs, validated the way naga validates it, and translated to GLSL
    /// ES 3.0 like the WebGL2 fallback does.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn the_shader_is_valid_for_webgpu_and_webgl2() {
        let module = naga::front::wgsl::parse_str(SHADER).unwrap();
        let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty()).validate(&module).unwrap();
        for (stage, entry) in [(naga::ShaderStage::Vertex, "vs"), (naga::ShaderStage::Fragment, "fs")] {
            let options = naga::back::glsl::Options { version: naga::back::glsl::Version::new_gles(300), ..Default::default() };
            let pipeline = naga::back::glsl::PipelineOptions { shader_stage: stage, entry_point: entry.into(), multiview: None };
            let mut out = String::new();
            let mut w =
                naga::back::glsl::Writer::new(&mut out, &module, &info, &options, &pipeline, naga::proc::BoundsCheckPolicies::default()).unwrap();
            w.write().unwrap();
            assert!(out.contains("#version 300 es"), "{entry}");
        }
    }

    /// The side the shader asks for: the requested size rounded to a whole pixel and never below
    /// one. Depth changes the colour, not the size.
    fn side(requested: f32) -> f32 {
        (requested + 0.5).floor().max(1.0)
    }

    /// The shader's centre snap, replicated in NDC: to device pixels, onto the half-pixel grid, and
    /// back again.
    fn snap(ndc: f32, viewport: f32) -> f32 {
        let pixels = ndc * viewport * 0.5;
        (pixels.floor() + 0.5) / (viewport * 0.5)
    }

    /// The four corners of the screen-space square, in device pixels, built from the centre in NDC,
    /// the viewport in device pixels and the side in device pixels.
    fn square(centre: [f32; 2], viewport: [f32; 2], side: f32) -> [[f32; 2]; 4] {
        let x = snap(centre[0], viewport[0]);
        let y = snap(centre[1], viewport[1]);
        let dx = side / viewport[0];
        let dy = side / viewport[1];
        let mut out = [[0.0; 2]; 4];
        for (corner, (sx, sy)) in out.iter_mut().zip([(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]) {
            *corner = [(x + sx * dx) * viewport[0] * 0.5, (y + sy * dy) * viewport[1] * 0.5];
        }
        out
    }

    /// The square covers exactly the requested number of device pixels on both axes, and its centre
    /// sits on the half-pixel grid, so no sub-pixel camera motion changes which pixels it covers.
    #[test]
    fn the_point_square_is_whole_pixels_wide_and_centred_on_a_half_pixel() {
        assert_eq!(side(3.0), 3.0);
        assert_eq!(side(10.0), 10.0, "no depth shrink: the level's spacing is what is drawn");
        assert_eq!(side(1.5), 2.0, "a pixel and a half rounds up, so the squares tile the cloud");
        assert_eq!(side(0.2), 1.0, "never below one pixel");

        let viewport = [1600.0, 1000.0];
        // A centre that lands exactly on the corner of four device pixels.
        let corners = square([400.0 / 800.0, 300.0 / 500.0], viewport, 3.0);
        // Device pixels are floats in the shader, so the corners are pinned to a thousandth of one.
        let close = |a: f32, b: f32| (a - b).abs() < 1e-3;
        assert!(close(corners[0][0], 399.0) && close(corners[0][1], 299.0), "{corners:?}");
        assert!(close(corners[1][0], 402.0) && close(corners[1][1], 299.0), "{corners:?}");
        assert!(close(corners[2][0], 399.0) && close(corners[2][1], 302.0), "{corners:?}");
        assert!(close(corners[3][0], 402.0) && close(corners[3][1], 302.0), "{corners:?}");
        assert!(close((corners[1][0] - corners[0][0]).abs(), 3.0), "3 device pixels wide");
        assert!(close((corners[2][1] - corners[0][1]).abs(), 3.0), "3 device pixels high");
        let centre = [(corners[0][0] + corners[3][0]) * 0.5, (corners[0][1] + corners[3][1]) * 0.5];
        assert!(close(centre[0], 400.5) && close(centre[1], 300.5), "{centre:?}");
    }
}
