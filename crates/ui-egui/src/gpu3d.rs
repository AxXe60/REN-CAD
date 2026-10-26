//! 3D point cloud renderer: a perspective camera over the session's clouds.
//!
//! Unlike the 2D canvas, the geometry here is world-space 3D and does not change when the camera
//! moves, so the vertex buffer is uploaded once per level/colour change and each frame only the
//! 160-byte view-projection uniform is written. Points are drawn as 6-vertex screen-space squares
//! (instanced, one instance per point): a `PointList` would need `gl_PointSize`, which WebGPU's
//! WGSL does not have.
//!
//! The pipeline lives in egui's callback resources, next to the 2D canvas pipelines, and shares
//! its target format and depth-stencil attachment. The 2D canvas only writes the stencil half of
//! that buffer (`depth_compare` is `Always`, `depth_write_enabled` false), so the clouds can own
//! the depth half: each square writes its own view depth and a nearer square hides the ones behind
//! it. Two things help where the buffer alone cannot:
//!
//! - the **front wall cut**: the points between the eye and a given distance are dropped, so the
//!   camera can look into a room without cropping the walls out of the cloud;
//! - the **depth cue**: the further a point is behind the orbit target, the more it fades into the
//!   background, so the near geometry stands out from the back wall that would otherwise be mixed
//!   into it. It changes the colour only: the squares stay the size the level asked for, so the
//!   cloud keeps its shape at every distance.

use std::sync::{Arc, Mutex};

use egui_wgpu::wgpu;
use egui_wgpu::wgpu::util::DeviceExt;

use crate::gpu::GpuTarget;

/// `[x, y, z]` f32 + `size` f32 + RGBA8: 20 bytes per point.
pub const VERTEX3D_SIZE: usize = 20;
/// Points in the uploaded scene. Only the part of the scan near the orbit target is uploaded, so
/// this is the visible budget, not the cloud size: 3M points is 60 MB and still a small vertex load
/// for a frame, and at deep zoom the full-resolution level fits inside it.
pub const MAX_SCENE_POINTS: usize = 3_000_000;

/// The per-frame uniform of the 3D pipeline. The layout is `repr(C)` with nothing but `f32`
/// arrays, so the struct is cast into the buffer as it is (`bytemuck`).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct View3dUniform {
    /// Column-major world → clip matrix.
    pub view_proj: [f32; 16],
    /// x: the smallest square the user asked for, in device pixels, y/z: the callback panel in
    /// device pixels (its borders are [-1, +1]), w: linear target.
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
    /// x: whether the x-ray mask is on; y: the display's device pixels per point, which turns the
    /// squares the scene carries (in points) into the pixels the target measures; the rest of the
    /// vector is the padding WGSL wants. Each point carries its own opacity in its alpha, so a
    /// uniform has no business holding it.
    pub misc: [f32; 4],
}

const UNIFORM3D_SIZE: u64 = std::mem::size_of::<View3dUniform>() as u64;

/// One point as the GPU sees it: its world position, the square in points its own level asks for,
/// and the colour of its own cloud — with that cloud's opacity already in the alpha.
#[derive(Clone, Copy, Debug)]
pub struct Point3d {
    pub pos: [f32; 3],
    pub color: [u8; 4],
    pub size: f32,
}

/// One splat's square as six corners, in clip-space offsets: two counter-clockwise triangles over
/// A = (-1,-1), B = (1,-1), C = (1,1), D = (-1,1) — A,B,C then A,C,D. The mapping the shader used to
/// derive these from `@builtin(vertex_index)` put C and D in the wrong corners, so both triangles
/// covered the same lower-left half and a point drew half the pixels its level asked for.
const QUAD: [[f32; 2]; 6] = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];

/// The instance buffer: one 20-byte record per point, in the order the attributes are declared.
fn pack_points(points: &[Point3d]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(points.len() * VERTEX3D_SIZE);
    for p in points {
        for v in p.pos {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes.extend_from_slice(&p.size.to_le_bytes());
        bytes.extend_from_slice(&p.color);
    }
    bytes
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
    /// Where the cloud is painted: the callback rect, in points. Its borders are the [-1, +1] of
    /// normalized device coordinates, so it is the panel, not the window.
    pub viewport: egui::Rect,
    /// The smallest point square the user asked for, in points. Each point carries the square its
    /// own level asks for; this is only the floor, and it is live per frame, so the Size slider
    /// never rebuilds the scene.
    pub min_point_size: f32,
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
    /// Thin the points by a deterministic mask on the instance index, so a wall seen edge-on does
    /// not become a solid slab of pixels when the cloud is semi-transparent.
    pub xray: bool,
    /// The render pass's depth-stencil format, so the pipeline matches it: its stencil is what the
    /// 2D canvas clips viewports with, its depth is what this pipeline sorts the clouds with.
    pub stencil: Option<wgpu::TextureFormat>,
}

struct Resources3d {
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// The six corners of the splat square, drawn once per instance. It is a buffer rather than
    /// arithmetic on `@builtin(vertex_index)` so the corners the unit test checks are the ones the
    /// GPU actually reads.
    quad: wgpu::Buffer,
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
        const ATTRS: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32, 2 => Unorm8x4];
        let instance_layout =
            wgpu::VertexBufferLayout { array_stride: VERTEX3D_SIZE as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &ATTRS };
        const CORNER_ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![3 => Float32x2];
        let quad_layout = wgpu::VertexBufferLayout { array_stride: 8, step_mode: wgpu::VertexStepMode::Vertex, attributes: &CORNER_ATTRS };
        let quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cad_cloud3d_quad"),
            contents: bytemuck::cast_slice(&QUAD),
            usage: wgpu::BufferUsages::VERTEX,
        });
        // The pass carries the window's depth-stencil buffer when it has one: the clouds write the
        // depth of every point they draw and test it before drawing another, so the scan nearest the
        // eye is what the user sees whatever order the clouds were opened in. The stencil half
        // belongs to the 2D canvas and is left alone. A target with no depth aspect (a `Stencil8`
        // test target, or wasm without a depth buffer) keeps the old front-to-back-free drawing,
        // where only the x-ray mask separates the near wall from the far one.
        let depth_stencil = target.stencil.map(|format| {
            let depth = format.has_depth_aspect();
            wgpu::DepthStencilState {
                format,
                depth_write_enabled: depth.then_some(true),
                depth_compare: depth.then_some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cad_cloud3d"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                buffers: &[Some(instance_layout), Some(quad_layout)],
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
        Self { pipeline, uniform, bind_group, quad, buffer: None, count: 0, key: None, linear_out: target.format.is_srgb() }
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
            // The scene is read, not moved: a click walks these same points to find the cloud under
            // the cursor, and the slot is only refilled when the key changes, so an upload that
            // swallowed them left a still camera — the one the user clicks with — with nothing to
            // snap to.
            let held = self.slot.lock().ok();
            if let Some(scene) = held.as_ref().and_then(|s| s.as_ref()) {
                if scene.points.is_empty() {
                    res.buffer = None;
                    res.count = 0;
                } else {
                    res.buffer = Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("cad_cloud3d_points"),
                        contents: &pack_points(&scene.points),
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    }));
                    res.count = scene.points.len() as u32;
                }
                res.key = Some(self.key);
            }
        }
        let ppp = screen.pixels_per_point;
        let linear = if res.linear_out { 1.0f32 } else { 0.0 };
        // The callback rect in device pixels — the same box `paint` sets as the pass's viewport, and
        // therefore the box whose borders are [-1, +1]. The whole-pixel snap and a square's side are
        // measured against it, so a square the shader rounds to N device pixels covers exactly N.
        let panel = egui::PaintCallbackInfo {
            viewport: self.viewport,
            clip_rect: self.viewport,
            pixels_per_point: ppp,
            screen_size_px: screen.size_in_pixels,
        }
        .viewport_in_pixels();
        let uniform = View3dUniform {
            view_proj: self.view_proj,
            params: [self.min_point_size * ppp, panel.width_px as f32, panel.height_px as f32, linear],
            clip: [self.near_cut, self.cue_span, self.target_depth, self.cue],
            eye: [self.eye[0], self.eye[1], self.eye[2], 0.0],
            forward: [self.forward[0], self.forward[1], self.forward[2], 0.0],
            bg: self.bg,
            misc: [if self.xray { 1.0 } else { 0.0 }, ppp, 0.0, 0.0],
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
        let panel = info.viewport_in_pixels();
        if panel.width_px <= 0 || panel.height_px <= 0 {
            return;
        }
        // Paint into the panel the callback was given, which egui-wgpu has already set: claiming the
        // whole window instead would move the cloud to the window's centre and scale every square by
        // the window over the panel.
        pass.set_viewport(panel.left_px as f32, panel.top_px as f32, panel.width_px as f32, panel.height_px as f32, 0.0, 1.0);
        pass.set_pipeline(&res.pipeline);
        pass.set_bind_group(0, &res.bind_group, &[]);
        pass.set_vertex_buffer(0, buffer.slice(..));
        pass.set_vertex_buffer(1, res.quad.slice(..));
        // Six vertices per instance: two triangles make the square.
        pass.draw(0..6, 0..res.count);
    }
}

const SHADER: &str = r#"
struct View {
    view_proj: mat4x4<f32>,
    // x: the smallest square the user asked for, y/z: the panel in device pixels, w: linear target
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
    // x: the x-ray mask on, y: device pixels per point; the rest is padding. Each point brings its
    // own opacity in its alpha.
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
    @builtin(instance_index) ii: u32,
    @location(0) p: vec3<f32>,
    @location(1) size: f32,
    @location(2) c: vec4<f32>,
    @location(3) corner: vec2<f32>,
) -> VOut {
    var clip = view.view_proj * vec4<f32>(p, 1.0);
    // The depth of the point along the view direction: the distance from the eye. `clip.w` would
    // do it in perspective, but the orthographic projection leaves it at 1.
    let depth = dot(p - view.eye.xyz, view.forward.xyz);
    var out: VOut;
    // The x-ray: with a semi-transparent cloud the points are thinned by a deterministic mask on
    // the instance index, so a wall seen edge-on does not turn into a solid slab of pixels and the
    // drawing under the cloud still shows through the gaps. Depth sorting keeps the near wall in
    // front, but only the mask lets anything behind it be seen at all. The cloud's own
    // opacity is this point's alpha: it is what the mask keeps a fraction of.
    let opacity = c.a;
    if (view.misc.x > 0.5 && opacity < 0.999) {
        let h = (ii * 2654435761u >> 16) & 0xFFu;
        if (h >= u32(opacity * 255.0 + 0.5)) {
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
    // background (up to 45%). The ramp is quadratic, so the points just behind the target keep
    // their colour and only the far ones recede.
    let span = max(view.clip.y, 0.0001);
    let t = clamp((depth - view.clip.z) / span, 0.0, 1.0);
    let far = t * t;
    let cue = view.clip.w * far;
    // The colour is premultiplied: the opacity scales the colour and its alpha together.
    var col = vec4<f32>(mix(c.rgb, view.bg.rgb, cue * 0.45) * opacity, c.a * opacity);
    // The square is drawn at a whole-pixel size, rounding the size the level asks for and never
    // going below one pixel, and its centre is snapped to a half pixel. A fractional square at a
    // fractional position covers a different set of pixels for every sub-pixel of camera motion,
    // which made the cloud crawl while orbiting. The size does not shrink with depth: the depth cue
    // only fades the colour, because a far square that shrank as well fell to a single pixel and
    // left gaps wherever the level drew its points close together.
    // The scene speaks points and the target measures device pixels, so the level's square is
    // converted before it is rounded: left in points, a 2× display drew every point at half the size
    // its level asked for and the slider's floor, which is in device pixels, swallowed the spacing.
    // The old half-extent was half of the square asked for, so the points were sub-pixel and dim.
    let side = max(1.0, floor(max(size * view.misc.y, view.params.x) + 0.5));
    let half_viewport = vec2<f32>(view.params.y * 0.5, view.params.z * 0.5);
    if (clip.w > 0.0) {
        // The pass's viewport is the panel in device pixels, so half of it spans the [-1, 1] range.
        let centre = clip.xy / clip.w * half_viewport;
        let snapped = floor(centre) + vec2<f32>(0.5);
        clip.x = snapped.x / half_viewport.x * clip.w;
        clip.y = snapped.y / half_viewport.y * clip.w;
    }
    // A square of `side` device pixels needs an NDC half-extent of `side / params.y` on x (and
    // `side / params.z` on y), times `clip.w` to get back into clip space. `corner` is the template
    // buffer's own corner, so the point covers the whole square in both triangles.
    let extent = vec2<f32>(side / view.params.y, side / view.params.z);
    clip.x += corner.x * extent.x * clip.w;
    clip.y += corner.y * extent.y * clip.w;
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
    /// member must sit where WGSL expects it (16-byte aligned, 160 bytes in all).
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
            misc: [1.0, 2.0, 0.0, 0.0],
        };
        let bytes = bytemuck::bytes_of(&u);
        let at = |offset: usize| f32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]]);
        assert_eq!(at(64), 1.0, "params");
        assert_eq!(at(80), 5.0, "clip");
        assert_eq!(at(96), 9.0, "eye");
        assert_eq!(at(112), 12.0, "forward");
        assert_eq!(at(128), 0.25, "bg");
        assert_eq!(at(144), 1.0, "the x-ray mask");
        assert_eq!(at(148), 2.0, "the pixels per point, which is what turns a level's square into device pixels");
        for offset in [0, 64, 80, 96, 112, 128, 144] {
            assert_eq!(offset % 16, 0, "every member is 16-byte aligned");
        }
    }

    /// Every point carries its own square and its own alpha: two clouds drawn at different levels
    /// have different spacings, and each cloud has its own opacity, so neither can be a uniform.
    /// The instance is `[x, y, z]` f32, `size` f32, RGBA8 — 20 bytes, packed in that order.
    #[test]
    fn every_point_carries_its_own_square_and_alpha() {
        assert_eq!(std::mem::size_of::<Point3d>(), 20);
        assert_eq!(VERTEX3D_SIZE, 20);
        let bytes = pack_points(&[
            Point3d { pos: [1.0, 2.0, 3.0], color: [10, 20, 30, 255], size: 2.5 },
            Point3d { pos: [-4.0, 5.0, 0.0], color: [0, 0, 0, 128], size: 1.5 },
        ]);
        assert_eq!(bytes.len(), 40, "one stride per point");
        let f = |offset: usize| f32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]]);
        assert_eq!(f(0), 1.0);
        assert_eq!(f(4), 2.0);
        assert_eq!(f(8), 3.0);
        assert_eq!(f(12), 2.5, "the square this point's level asks for");
        assert_eq!(&bytes[16..20], &[10, 20, 30, 255], "the cloud's opacity is in the alpha");
        assert_eq!(f(20), -4.0, "the next point starts at the stride");
        assert_eq!(f(32), 1.5);
        assert_eq!(&bytes[36..40], &[0, 0, 0, 128]);
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

    /// The rounding the shader applies once the square is in device pixels: to a whole pixel, never
    /// below one. Depth changes the colour, not the size.
    fn side(requested: f32) -> f32 {
        (requested + 0.5).floor().max(1.0)
    }

    /// Is the point inside the triangle? Signed-edge test, winding-agnostic, because the pipeline
    /// culls nothing.
    fn inside(triangle: &[[f32; 2]], p: [f32; 2]) -> bool {
        let edge = |a: [f32; 2], b: [f32; 2]| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
        let (d0, d1, d2) = (edge(triangle[0], triangle[1]), edge(triangle[1], triangle[2]), edge(triangle[2], triangle[0]));
        (d0 <= 0.0 && d1 <= 0.0 && d2 <= 0.0) || (d0 >= 0.0 && d1 >= 0.0 && d2 >= 0.0)
    }

    /// The template the GPU reads, checked as data rather than re-derived here: its corners are the
    /// square's, and the two triangles cover every sample inside it. The mapping the shader used to
    /// compute from `@builtin(vertex_index)` sent both triangles over the same lower-left half, and
    /// that is what this would catch — half the samples would fall in neither.
    #[test]
    fn the_six_vertices_tile_the_square() {
        assert!(QUAD.iter().all(|c| c[0].abs() == 1.0 && c[1].abs() == 1.0), "every corner is a corner of the square");
        let samples: Vec<[f32; 2]> = (-9i8..=9).flat_map(|i| (-9i8..=9).map(move |j| [f32::from(i) / 10.0, f32::from(j) / 10.0])).collect();
        let covered = samples.iter().filter(|p| inside(&QUAD[0..3], **p) || inside(&QUAD[3..6], **p)).count();
        assert_eq!(covered, samples.len(), "the two triangles cover the whole splat square");
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
