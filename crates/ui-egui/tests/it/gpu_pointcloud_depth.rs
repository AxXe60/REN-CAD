//! The 3D cloud pipeline draws against a real depth buffer, so the point nearest the eye is what
//! the user sees wherever two splats cover the same pixel — whatever order the clouds were opened
//! in. The window's depth attachment is shared with the 2D canvas's stencil, so the pipeline names
//! `Depth24PlusStencil8` and writes depth; without that, the last cloud in the scene silently
//! painted over every earlier one.
//!
//! The pipeline also has to draw *inside the panel it was given*: egui defines the [-1, +1] of
//! normalised device coordinates as the callback's rect, so a viewport of the whole window puts the
//! cloud over the toolbars and scales every square by the window.
//!
//! Renders into an offscreen colour + depth-stencil target and reads the pixels back, like
//! `gpu_viewport_stencil.rs` does for the 2D canvas.

use std::sync::{Arc, Mutex};

use cadcraft_ui_egui::gpu::{self, GpuFaults};
use cadcraft_ui_egui::gpu3d::{self, Cloud3dCallback, Point3d, Scene3d, SceneSlot};
use egui_wgpu::wgpu;
use egui_wgpu::{CallbackTrait, ScreenDescriptor};

const SIZE: u32 = 64;
/// What the window asks for: 24 depth bits and the 8 stencil bits the 2D canvas clips with.
const DEPTH_STENCIL: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

/// An orthographic world → clip matrix: x maps to the screen column, z to the screen row, and the
/// view depth (10..20 m along +Y, from an eye at y = −10) to `wgpu`'s 0..1 range, nearer being
/// smaller. Every splat therefore lands on the middle of its panel, at its own depth.
const VIEW_PROJ: [f32; 16] = [0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.025, 0.0, 0.0, 0.1, 0.0, 0.0, 0.0, 0.0, 0.25, 1.0];

/// A splat `y` metres along the view direction, on the middle of the panel.
fn splat(y: f32, color: [u8; 4], size: f32) -> Point3d {
    Point3d { pos: [0.0, y, 0.0], color, size }
}

/// A one-pixel splat `x` metres along the screen column, on the panel's middle row.
fn column(x: f32, color: [u8; 4]) -> Point3d {
    Point3d { pos: [x, 0.0, 0.0], color, size: 1.0 }
}

fn near_red() -> Point3d {
    splat(0.0, [255, 0, 0, 255], 9.0)
}

fn far_blue() -> Point3d {
    splat(8.0, [0, 0, 255, 255], 9.0)
}

/// One offscreen draw: the scene, the panel it is drawn into, and the two things the pipeline takes
/// from the uniform rather than from the vertex buffer — the user's size floor and the x-ray mask.
struct Draw {
    key: u64,
    panel: egui::Rect,
    screen: u32,
    floor: f32,
    xray: bool,
    ppp: f32,
    /// The scene's home, kept by the draw so a test can read what the upload leaves behind: the
    /// click snap walks these points, and it only gets a frame the callback does not clear.
    slot: SceneSlot,
}

impl Draw {
    /// A scene drawn into the whole 64×64 target, one device pixel to the point, at the smallest
    /// floor and with the mask off.
    fn new(points: Vec<Point3d>, key: u64) -> Self {
        Self {
            key,
            panel: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(SIZE as f32, SIZE as f32)),
            screen: SIZE,
            floor: 1.0,
            xray: false,
            ppp: 1.0,
            slot: Arc::new(Mutex::new(Some(Scene3d { points }))),
        }
    }

    /// The same scene on a HiDPI display: the panel is `size / ppp` points wide, so it still fills
    /// the target, and every square the scene asks for in points becomes `ppp` times as many device
    /// pixels.
    fn scaled(mut self, ppp: f32) -> Self {
        self.ppp = ppp;
        self.panel = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(SIZE as f32 / ppp, SIZE as f32 / ppp));
        self
    }
}

/// The pixels of a rendered target, row by row from the top.
struct Shot {
    width: u32,
    px: Vec<[u8; 3]>,
}

impl Shot {
    fn at(&self, x: u32, y: u32) -> [u8; 3] {
        self.px[(y * self.width + x) as usize]
    }

    /// The middle of the target, where a point at the centre of a full-target panel lands.
    fn centre(&self) -> [u8; 3] {
        self.at(self.width / 2, self.width / 2)
    }

    fn covered(&self) -> usize {
        self.px.iter().filter(|rgb| **rgb != [0, 0, 0]).count()
    }

    /// The columns that carry colour, left to right: which points survived is what the x-ray test
    /// reads, and two frames of one scene have to agree on it.
    fn lit_columns(&self) -> Vec<u32> {
        (0..self.width).filter(|x| (0..self.width).any(|y| self.at(*x, y) != [0, 0, 0])).collect()
    }
}

/// Render `d` into a fresh colour + depth-stencil target and read its pixels back.
fn render(rs: &egui_wgpu::RenderState, d: &Draw) -> Shot {
    let (device, queue) = (&rs.device, &rs.queue);
    let extent = wgpu::Extent3d { width: d.screen, height: d.screen, depth_or_array_layers: 1 };
    let texture = |format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let color = texture(rs.target_format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
    let depth_stencil = texture(DEPTH_STENCIL, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let (color_view, ds_view) = (color.create_view(&Default::default()), depth_stencil.create_view(&Default::default()));

    let cb = Cloud3dCallback {
        key: d.key,
        slot: d.slot.clone(),
        view_proj: VIEW_PROJ,
        viewport: d.panel,
        min_point_size: d.floor,
        eye: [0.0, -10.0, 0.0],
        forward: [0.0, 1.0, 0.0],
        near_cut: 0.0,
        cue_span: 40.0,
        target_depth: 10.0,
        cue: 0.0,
        bg: [0.0, 0.0, 0.0, 1.0],
        xray: d.xray,
        stencil: Some(DEPTH_STENCIL),
    };

    let screen = ScreenDescriptor { size_in_pixels: [d.screen, d.screen], pixels_per_point: d.ppp };
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut renderer = rs.renderer.write();
        let _ = cb.prepare(device, queue, &screen, &mut encoder, &mut renderer.callback_resources);
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &ds_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
                }),
                ..Default::default()
            })
            .forget_lifetime();
        let info = egui::PaintCallbackInfo { viewport: d.panel, clip_rect: d.panel, pixels_per_point: d.ppp, screen_size_px: [d.screen, d.screen] };
        cb.paint(info, &mut pass, &renderer.callback_resources);
    }
    let row = d.screen * 4;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row * d.screen),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        color.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(d.screen) },
        },
        extent,
    );
    queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(std::time::Duration::from_secs(10)) }).unwrap();
    let pixels = buffer.slice(..).get_mapped_range().unwrap();
    Shot { width: d.screen, px: pixels.chunks(4).map(|p| [p[0], p[1], p[2]]).collect() }
}

fn is_red(rgb: &[u8; 3]) -> bool {
    rgb[0] > 200 && rgb[1] < 60 && rgb[2] < 60
}

fn is_blue(rgb: &[u8; 3]) -> bool {
    rgb[2] > 200 && rgb[0] < 60 && rgb[1] < 60
}

fn setup() -> (egui_wgpu::RenderState, Arc<GpuFaults>) {
    let rs = egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), Default::default());
    let faults = Arc::new(GpuFaults::default());
    let target = gpu::install(&rs, &faults, Some(DEPTH_STENCIL));
    assert_eq!(target.stencil, Some(DEPTH_STENCIL), "the 2D canvas keeps its stencil, so clouds share the buffer");
    gpu3d::install(&rs, target);
    (rs, faults)
}

/// The nearer splat wins wherever it covers a pixel, whichever order the points were uploaded in:
/// the depth buffer, not the scene order, decides what the user sees.
#[test]
fn the_near_splat_hides_the_far_one_in_either_order() {
    let (rs, faults) = setup();
    // The scanner's file order: the far cloud first, then the near one on top of it.
    let shot = render(&rs, &Draw::new(vec![far_blue(), near_red()], 1));
    assert!(is_red(&shot.centre()), "far first: {:?}", shot.centre());
    // And the same scene with the two clouds swapped: opening a second scan must not change what
    // the first one looks like behind it.
    let shot = render(&rs, &Draw::new(vec![near_red(), far_blue()], 2));
    assert!(is_red(&shot.centre()), "near first: {:?}", shot.centre());
    assert!(!is_blue(&shot.centre()), "the far cloud does not bleed through");
    assert_eq!(faults.take(), None, "wgpu reported nothing");
}

/// Depth only hides what is behind it: the pixels the near splat does not reach are still painted
/// by the far one, and the pixels no splat covers keep the background.
#[test]
fn the_far_splat_still_draws_where_the_near_one_does_not_reach() {
    let (rs, faults) = setup();
    // 9 px about the centre (30..38) inside a 25 px far square (19..44).
    let shot = render(&rs, &Draw::new(vec![near_red(), splat(8.0, [0, 0, 255, 255], 25.0)], 3));
    assert!(is_red(&shot.centre()), "the near square covers the centre: {:?}", shot.centre());
    assert!(is_blue(&shot.at(22, 32)), "outside it the far cloud still paints: {:?}", shot.at(22, 32));
    assert_eq!(shot.at(0, 0), [0, 0, 0], "untouched by either");
    assert_eq!(faults.take(), None);
}

/// Each point is drawn as the square its own cloud asked for, not one global size: a 1 px splat
/// covers one device pixel and a 5 px splat covers twenty-five.
#[test]
fn every_point_covers_the_square_it_asks_for() {
    let (rs, faults) = setup();
    let covered = |points, key| render(&rs, &Draw::new(points, key)).covered();
    assert_eq!(covered(vec![splat(0.0, [255, 255, 255, 255], 1.0)], 4), 1, "a one-pixel splat");
    assert_eq!(covered(vec![splat(0.0, [255, 255, 255, 255], 5.0)], 5), 25, "a five-pixel splat");
    assert_eq!(faults.take(), None);
}

/// A semi-transparent cloud writes depth like an opaque one: what it covers, it hides, so the scan
/// behind it does not add its colour through the front wall and turn the room into a mush.
#[test]
fn a_semi_transparent_cloud_still_hides_what_is_behind_it() {
    let (rs, faults) = setup();
    // alpha 128 over an opaque blue splat on the same pixel: the blend lands on the background
    // alone, because the far point never passes the depth test.
    let shot = render(&rs, &Draw::new(vec![splat(0.0, [128, 0, 0, 128], 9.0), far_blue()], 7));
    let rgb = shot.centre();
    assert!(rgb[0] > rgb[2], "the far cloud must not add its blue through the near one: {rgb:?}");
    assert!(rgb[2] < 60, "and it is not merely dimmed: {rgb:?}");
    assert_eq!(faults.take(), None);
}

/// egui defines the [-1, +1] of normalised device coordinates as the callback's own rect, so the
/// pipeline renders into that viewport and never into the whole window: a point at the panel centre
/// belongs at the panel centre, and a five-pixel square is five device pixels wide there. Painting
/// against the window put the cloud where the toolbars are and every square twice as big.
#[test]
fn the_cloud_lands_in_the_panel_it_was_given() {
    let (rs, faults) = setup();
    // A panel on the right of a 128 px window, along its top edge: its centre is (96, 16), which is
    // nowhere near the window's centre at (64, 64).
    let mut d = Draw::new(vec![splat(0.0, [255, 255, 255, 255], 5.0)], 11);
    d.panel = egui::Rect::from_min_max(egui::pos2(64.0, 0.0), egui::pos2(128.0, 32.0));
    d.screen = 128;
    let shot = render(&rs, &d);
    assert_eq!(shot.at(96, 16), [255, 255, 255], "the panel centre carries the point");
    assert_eq!(shot.at(64, 64), [0, 0, 0], "the window centre is not the panel centre");
    assert_eq!(shot.covered(), 25, "five device pixels wide, not five points scaled to the window");
    assert_eq!(faults.take(), None);
}

/// The Size slider is live: it is the floor carried by the uniform, so moving it costs one 160-byte
/// write and no vertex rebuild. One key, one uploaded scene, two floors — and the same points drawn
/// at the floor each frame asks for. A floor baked into the scene would keep the points fat after
/// the user asks for thin ones, because the scene key does not change with the slider.
#[test]
fn the_size_floor_is_applied_live_not_baked() {
    let (rs, faults) = setup();
    let points = vec![splat(0.0, [255, 255, 255, 255], 1.0)];
    let shot = |floor: f32| {
        let mut d = Draw::new(points.clone(), 21);
        d.floor = floor;
        render(&rs, &d)
    };
    assert_eq!(shot(1.0).covered(), 1, "a one-pixel point at the smallest floor");
    assert_eq!(shot(5.0).covered(), 25, "the same scene under the same key, at the floor the user just asked for");
    assert_eq!(faults.take(), None);
}

/// The points survive their own upload: the callback packs them for the GPU but leaves the scene in
/// the slot, because that is what a click reads to find the cloud point under the cursor. Stealing
/// them there meant a still camera — the one the user clicks with — had nothing to snap to.
#[test]
fn the_upload_leaves_the_scene_for_the_click_snap() {
    let (rs, faults) = setup();
    let d = Draw::new(vec![near_red(), far_blue()], 41);
    let shot = render(&rs, &d);
    assert!(shot.covered() > 0, "the scene really was drawn");
    let kept = d.slot.lock().map(|s| s.as_ref().map_or(0, |scene| scene.points.len())).unwrap_or(0);
    assert_eq!(kept, 2, "the slot still holds both points after the upload");
    assert_eq!(faults.take(), None);
}

/// On a HiDPI display the squares the levels ask for grow with the display, not only the floor the
/// slider does: a scene speaks points and the target measures device pixels, so a two-pixel-per-point
/// panel has to multiply. Drawn at half scale, every cloud on such a screen turns into a haze of
/// single pixels and the slider's floor swallows the level's own spacing.
#[test]
fn a_point_covers_the_square_its_level_asked_for_in_physical_pixels() {
    let (rs, faults) = setup();
    // Two device pixels to the point: the 32-point panel fills the 64-pixel target.
    let one = Draw::new(vec![splat(0.0, [255, 255, 255, 255], 1.0)], 51).scaled(2.0);
    assert_eq!(render(&rs, &one).covered(), 4, "one point is four device pixels on a 2× display");
    let fat = Draw::new(vec![splat(0.0, [255, 255, 255, 255], 2.5)], 52).scaled(2.0);
    assert_eq!(render(&rs, &fat).covered(), 25, "two and a half points are a five-pixel square");
    assert_eq!(faults.take(), None);
}

/// The mask's fraction is of the cloud's opacity, and the panel's size is of the display: the two
/// together must not make the floor swallow the level on a 2× screen, where the floor of one point is
/// already two device pixels.
#[test]
fn the_floor_and_the_level_size_agree_on_a_hidpi_display() {
    let (rs, faults) = setup();
    // The level asks for 1.5 points (the clamp's floor) and the slider for 1 point: at 2× the point
    // is 3 device pixels wide, and at 1× it would be 1.5 rounded to 2.
    let d = Draw::new(vec![splat(0.0, [255, 255, 255, 255], 1.5)], 61).scaled(2.0);
    assert_eq!(render(&rs, &d).covered(), 9, "the level's square, in device pixels");
    assert_eq!(faults.take(), None);
}

/// The x-ray mask thins a semi-transparent cloud by a rule fixed on the instance index, so one
/// scene draws the same points every frame — an orbit must not make a wall shimmer — and it leaves
/// an opaque cloud whole, because there is nothing to see through it.
#[test]
fn the_x_ray_mask_thins_only_the_semi_transparent_cloud() {
    let (rs, faults) = setup();
    // Seventeen one-pixel points, one every 3.2 device pixels across the panel.
    let row = |alpha| (0..17).map(|i| column(i as f32 - 8.0, [255, 255, 255, alpha])).collect::<Vec<_>>();
    let xray = |alpha: u8, key: u64| {
        let mut d = Draw::new(row(alpha), key);
        d.xray = true;
        d
    };
    let whole = Draw::new(row(128), 32);
    assert_eq!(render(&rs, &whole).lit_columns().len(), 17, "the mask is off: every point is drawn");
    let lit = render(&rs, &xray(128, 31)).lit_columns();
    let n = lit.len();
    assert!(n > 4 && n < 14, "roughly the cloud's own fraction survives, and none of it vanishes: {n} of 17");
    assert_eq!(lit, render(&rs, &xray(128, 31)).lit_columns(), "the same scene keeps the same points");
    assert_eq!(render(&rs, &xray(255, 33)).lit_columns().len(), 17, "an opaque cloud is not perforated");
    assert_eq!(faults.take(), None);
}
