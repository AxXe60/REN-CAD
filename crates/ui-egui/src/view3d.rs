//! The 3D view: an orbit camera over the session's point clouds.
//!
//! The drawing canvas is 2D; when the 3D view is on, [`show`] replaces it inside the same rect
//! and handles its own input (drag orbits, middle/shift-drag pans, wheel dollies, double-click
//! fits). The points come from the clouds' existing decimation pyramid, picked by the on-screen
//! density, capped at [`MAX_SCENE_POINTS`], and drawn by [`crate::gpu3d`] — one upload per level
//! or colour change, then one instanced draw call per frame.

use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use cadcraft_pointcloud::CloudColor;
use egui::{Pos2, Rect, Sense, pos2, vec2};

use crate::CadApp;
use crate::gpu3d::{MAX_SCENE_POINTS, Point3d, Scene3d, SceneSlot};
use crate::theme::Tokens;

/// Orbit camera state; world Z is up (the scanner's vertical).
#[derive(Debug)]
pub struct View3d {
    pub active: bool,
    /// Point the camera turns around.
    pub target: [f64; 3],
    /// Radians around Z.
    pub yaw: f64,
    /// Radians above the XY plane, clamped short of the poles.
    pub pitch: f64,
    /// Eye distance to `target`, world units.
    pub dist: f64,
    /// Vertical field of view, radians.
    pub fov_y: f64,
    /// Point square side, points.
    pub point_size: f32,
    /// Fit the camera to the clouds on the next frame.
    pub fitted: bool,
    /// The scene last handed to the GPU, and how many points it holds.
    pub key: Option<u64>,
    pub drawn: usize,
    slot: SceneSlot,
}

impl Default for View3d {
    fn default() -> Self {
        Self {
            active: false,
            target: [0.0; 3],
            yaw: -2.2,
            pitch: 0.55,
            dist: 30.0,
            fov_y: 50.0_f64.to_radians(),
            point_size: 1.5,
            fitted: false,
            key: None,
            drawn: 0,
            slot: std::sync::Arc::new(Mutex::new(None)),
        }
    }
}

impl View3d {
    pub fn scene_slot(&self) -> SceneSlot {
        std::sync::Arc::clone(&self.slot)
    }

    /// Eye position from the orbit angles.
    pub fn eye(&self) -> [f64; 3] {
        let cp = self.pitch.cos();
        [
            self.target[0] + self.dist * cp * self.yaw.cos(),
            self.target[1] + self.dist * cp * self.yaw.sin(),
            self.target[2] + self.dist * self.pitch.sin(),
        ]
    }

    /// Camera basis: forward (target − eye), right, up.
    fn basis(&self) -> ([f64; 3], [f64; 3], [f64; 3]) {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let forward = [-cp * cy, -cp * sy, -sp];
        let right = [-sy, cy, 0.0];
        let up = [-cy * sp, -sy * sp, cp];
        (forward, right, up)
    }

    /// Column-major world → clip matrix for a canvas of `aspect` width/height.
    pub fn view_proj(&self, aspect: f64, radius: f64) -> [f32; 16] {
        let eye = self.eye();
        let view = look_at(eye, self.target, [0.0, 0.0, 1.0]);
        let near = (self.dist * 0.01).max(1e-3);
        // No depth test, so the far plane only has to be beyond the scene.
        let far = (self.dist + radius * 4.0).max(1.0) * 1.5;
        let proj = perspective(self.fov_y as f32, aspect.max(1e-3) as f32, near as f32, far as f32);
        mul(&proj, &view)
    }

    /// Frame the given world box (or a default 10 m cube when empty).
    pub fn fit(&mut self, bounds: Option<([f64; 3], [f64; 3])>) {
        let Some((min, max)) = bounds else {
            self.target = [0.0, 0.0, 0.0];
            self.dist = 20.0;
            self.fitted = true;
            return;
        };
        self.target = [0.5 * (min[0] + max[0]), 0.5 * (min[1] + max[1]), 0.5 * (min[2] + max[2])];
        let dx = max[0] - min[0];
        let dy = max[1] - min[1];
        let dz = max[2] - min[2];
        let radius = 0.5 * (dx * dx + dy * dy + dz * dz).sqrt();
        self.dist = (radius.max(0.1) / (self.fov_y * 0.5).sin()) * 1.15;
        self.fitted = true;
    }
}

/// The box around every visible cloud (world coordinates).
fn cloud_bounds(app: &CadApp) -> Option<([f64; 3], [f64; 3])> {
    let mut it = app.session.clouds.iter().filter(|c| c.visible);
    let first = it.next()?;
    let (mut min, mut max) = (first.bounds.min, first.bounds.max);
    for c in it {
        for i in 0..3 {
            min[i] = min[i].min(c.bounds.min[i]);
            max[i] = max[i].max(c.bounds.max[i]);
        }
    }
    Some((min, max))
}

/// The level each visible cloud contributes at this density, coarsened to the point budget.
fn pick_levels(clouds: &[cadcraft_pointcloud::Cloud], px_per_m: f64) -> Vec<(&cadcraft_pointcloud::Cloud, &cadcraft_pointcloud::CloudLevel)> {
    let mut picks = Vec::new();
    for c in clouds.iter().filter(|c| c.visible) {
        let chosen = c.level_for(px_per_m);
        let level =
            if chosen.pts.len() > MAX_SCENE_POINTS { c.levels.iter().find(|l| l.pts.len() <= MAX_SCENE_POINTS).unwrap_or(chosen) } else { chosen };
        picks.push((c, level));
    }
    picks
}

/// What the GPU buffer was built from: names, colour modes and level cells. Camera moves do not
/// change it, so orbiting never re-uploads.
fn scene_key(picks: &[(&cadcraft_pointcloud::Cloud, &cadcraft_pointcloud::CloudLevel)]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (c, level) in picks {
        c.name.hash(&mut hasher);
        c.color.name().hash(&mut hasher);
        if let CloudColor::Single([r, g, b]) = c.color {
            (r, g, b).hash(&mut hasher);
        }
        level.cell.to_bits().hash(&mut hasher);
        level.pts.len().hash(&mut hasher);
        if let Some(clip) = &c.clip {
            clip.min.map(f64::to_bits).hash(&mut hasher);
            clip.max.map(f64::to_bits).hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// Recolour the picked levels and pack them for upload.
fn build_scene(picks: &[(&cadcraft_pointcloud::Cloud, &cadcraft_pointcloud::CloudLevel)]) -> Scene3d {
    let total: usize = picks.iter().map(|(_, level)| level.pts.len()).sum();
    let mut scene = Scene3d { points: Vec::with_capacity(total) };
    for (c, level) in picks {
        for v in &level.pts {
            let (x, y, z) = (f64::from(v.x) + c.origin[0], f64::from(v.y) + c.origin[1], f64::from(v.z));
            if c.clip.is_some_and(|clip| !clip.contains(x, y, z)) {
                continue;
            }
            let [r, g, b] = c.color_of(v);
            scene.points.push(Point3d { pos: [x as f32, y as f32, z as f32], color: [r, g, b, 255] });
        }
    }
    scene
}

/// Show the 3D view in `rect` (the canvas area). Called by [`crate::canvas::show`].
pub fn show(app: &mut CadApp, ui: &mut egui::Ui, rect: Rect) {
    let t = Tokens::get();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, t.canvas);
    app.canvas.rect = Some(rect);
    app.session.viewport_px = (f64::from(rect.width()).max(1.0), f64::from(rect.height()).max(1.0));

    if !app.view3d.fitted {
        let bounds = cloud_bounds(app);
        app.view3d.fit(bounds);
    }

    // ---------- input ----------
    let resp = ui.interact(rect, ui.id().with("view3d"), Sense::click_and_drag());
    let (shift, wheel, middle, pointer_delta, inside) = ui.input(|i| {
        (
            i.modifiers.shift,
            i.smooth_scroll_delta.y,
            i.pointer.middle_down(),
            i.pointer.delta(),
            i.pointer.hover_pos().is_some_and(|p| rect.contains(p)),
        )
    });
    if inside && wheel != 0.0 {
        app.view3d.dist = (app.view3d.dist * (-f64::from(wheel) * 0.0015).exp()).clamp(0.02, 1e7);
    }
    // Middle-drag panning is read from the pointer (egui's drag sense follows the primary button).
    let drag = if inside && middle && pointer_delta != egui::Vec2::ZERO {
        Some((pointer_delta, false))
    } else if resp.dragged() {
        Some((resp.drag_delta(), shift && resp.dragged_by(egui::PointerButton::Primary)))
    } else {
        None
    };
    if let Some((d, pan)) = drag {
        if pan {
            let k = 2.0 * app.view3d.dist * (app.view3d.fov_y * 0.5).tan() / f64::from(rect.height()).max(1.0);
            let (_f, right, up) = app.view3d.basis();
            for i in 0..3 {
                app.view3d.target[i] += (-right[i] * f64::from(d.x) + up[i] * f64::from(d.y)) * k;
            }
        } else {
            app.view3d.yaw -= f64::from(d.x) * 0.008;
            app.view3d.pitch = (app.view3d.pitch - f64::from(d.y) * 0.008).clamp(-1.5, 1.5);
        }
    }
    if resp.double_clicked() {
        let bounds = cloud_bounds(app);
        app.view3d.fit(bounds);
    }

    // ---------- scene ----------
    let height = f64::from(rect.height()).max(1.0);
    let px_per_m = height / (2.0 * app.view3d.dist * (app.view3d.fov_y * 0.5).tan()).max(1e-9);
    let picks = pick_levels(&app.session.clouds, px_per_m);
    let key = scene_key(&picks);
    if app.view3d.key != Some(key) {
        let started = std::time::Instant::now();
        let scene = build_scene(&picks);
        let drawn = scene.points.len();
        if let Ok(mut slot) = app.view3d.slot.lock() {
            *slot = Some(scene);
        }
        app.view3d.key = Some(key);
        app.view3d.drawn = drawn;
        log::info!("cloud 3d scene: {drawn} points in {:.1} ms", started.elapsed().as_secs_f64() * 1000.0);
    }

    let aspect = f64::from(rect.width()) / height;
    let radius = cloud_bounds(app).map_or(10.0, |(min, max)| {
        let d = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
        0.5 * (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(0.1)
    });
    let vp = app.view3d.view_proj(aspect, radius);
    if let Some(target) = app.canvas.gpu {
        painter.add(egui_wgpu::Callback::new_paint_callback(
            rect,
            crate::gpu3d::Cloud3dCallback {
                key,
                slot: app.view3d.scene_slot(),
                view_proj: vp,
                viewport: [rect.width(), rect.height()],
                point_size: app.view3d.point_size,
                stencil: target.stencil,
            },
        ));
    } else {
        // No wgpu render state (wasm without a GPU): project on the CPU, sampled.
        draw_cpu_fallback(&painter, rect, &vp, &app.view3d, &app.session.clouds, px_per_m);
    }

    // ---------- HUD ----------
    let clouds = app.session.clouds.iter().filter(|c| c.visible).count();
    let info = format!(
        "3D · {clouds} cloud{}, {} points\nDrag: orbit · Shift or middle drag: pan · Wheel: zoom · Double-click: fit",
        if clouds == 1 { "" } else { "s" },
        app.view3d.drawn
    );
    painter.text(rect.left_top() + vec2(10.0, 8.0), egui::Align2::LEFT_TOP, info, egui::FontId::proportional(12.0), t.text_dim);
}

fn draw_cpu_fallback(painter: &egui::Painter, rect: Rect, vp: &[f32; 16], view: &View3d, clouds: &[cadcraft_pointcloud::Cloud], px_per_m: f64) {
    let mut shapes = Vec::new();
    for c in clouds.iter().filter(|c| c.visible) {
        let level = c.level_for(px_per_m);
        let step = (level.pts.len() / 50_000).max(1);
        for v in level.pts.iter().step_by(step) {
            let p = [(f64::from(v.x) + c.origin[0]) as f32, (f64::from(v.y) + c.origin[1]) as f32, v.z];
            if c.clip.is_some_and(|clip| !clip.contains(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]))) {
                continue;
            }
            if let Some(at) = project(vp, p, rect) {
                let [r, g, b] = c.color_of(v);
                shapes.push(egui::Shape::rect_filled(
                    Rect::from_center_size(at, vec2(view.point_size, view.point_size)),
                    0.0,
                    egui::Color32::from_rgb(r, g, b),
                ));
            }
        }
    }
    painter.extend(shapes);
}

fn project(m: &[f32; 16], p: [f32; 3], rect: Rect) -> Option<Pos2> {
    let c = transform(m, p);
    if c[3] <= 1e-6 {
        return None;
    }
    let x = c[0] / c[3];
    let y = c[1] / c[3];
    if !(-1.0..=1.0).contains(&x) || !(-1.0..=1.0).contains(&y) {
        return None;
    }
    Some(pos2(rect.left() + (x + 1.0) * 0.5 * rect.width(), rect.top() + (1.0 - y) * 0.5 * rect.height()))
}

fn look_at(eye: [f64; 3], target: [f64; 3], up: [f64; 3]) -> [f32; 16] {
    let f = normalize(sub(target, eye));
    let s = normalize(cross(f, up));
    let u = cross(s, f);
    [
        s[0] as f32,
        u[0] as f32,
        -f[0] as f32,
        0.0,
        s[1] as f32,
        u[1] as f32,
        -f[1] as f32,
        0.0,
        s[2] as f32,
        u[2] as f32,
        -f[2] as f32,
        0.0,
        -dot(s, eye) as f32,
        -dot(u, eye) as f32,
        dot(f, eye) as f32,
        1.0,
    ]
}

/// Right-handed perspective, WebGPU clip depth 0..1, column-major.
fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> [f32; 16] {
    let f = 1.0 / (fov_y * 0.5).tan();
    [f / aspect, 0.0, 0.0, 0.0, 0.0, f, 0.0, 0.0, 0.0, 0.0, far / (near - far), -1.0, 0.0, 0.0, (near * far) / (near - far), 0.0]
}

fn mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut out = [0.0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[k * 4 + row] * b[col * 4 + k];
            }
            out[col * 4 + row] = sum;
        }
    }
    out
}

fn transform(m: &[f32; 16], p: [f32; 3]) -> [f32; 4] {
    let mut out = [0.0f32; 4];
    for row in 0..4 {
        out[row] = m[row] * p[0] + m[4 + row] * p[1] + m[8 + row] * p[2] + m[12 + row];
    }
    out
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn normalize(a: [f64; 3]) -> [f64; 3] {
    let len = dot(a, a).sqrt();
    if len > 1e-12 { [a[0] / len, a[1] / len, a[2] / len] } else { [0.0, 0.0, 1.0] }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_view() -> View3d {
        let mut v = View3d { active: true, ..Default::default() };
        v.yaw = 0.0;
        v.pitch = 0.0;
        v.dist = 10.0;
        v.target = [1.0, 2.0, 3.0];
        v
    }

    #[test]
    fn the_target_projects_to_the_centre_of_the_canvas() {
        let v = default_view();
        let m = v.view_proj(1.6, 5.0);
        let c = transform(&m, [1.0, 2.0, 3.0]);
        assert!(c[3] > 0.0, "in front of the camera: {c:?}");
        assert!(c[0].abs() < 1e-4 && c[1].abs() < 1e-4, "centre: {c:?}");
    }

    #[test]
    fn z_is_up_on_screen_and_distance_shrinks_projection() {
        let v = default_view();
        let m = v.view_proj(1.0, 5.0);
        let below = transform(&m, [1.0, 2.0, 2.0]);
        let above = transform(&m, [1.0, 2.0, 4.0]);
        assert!(above[1] > below[1], "higher world Z is higher on screen");
        // The camera sits on +X looking towards -X, so a smaller X is farther away.
        let far = transform(&m, [1.0 - 5.0, 2.0, 3.0]);
        assert!(far[3] > transform(&m, [1.0, 2.0, 3.0])[3] + 1.0, "farther points have larger w");
    }

    #[test]
    fn fit_frames_the_cloud_box() {
        let mut v = default_view();
        v.fit(Some(([-5.0, -5.0, 0.0], [5.0, 5.0, 3.0])));
        assert_eq!(v.target, [0.0, 0.0, 1.5]);
        assert!(v.dist > 10.0 && v.dist < 40.0, "a 10 m box fits: {}", v.dist);
        v.fit(None);
        assert_eq!(v.target, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn an_empty_cloud_selection_builds_an_empty_scene() {
        let picks = pick_levels(&[], 50.0);
        let scene = build_scene(&picks);
        assert_eq!(scene.points.len(), 0);
        assert_eq!(scene_key(&picks), scene_key(&[]));
    }

    #[test]
    fn projection_puts_a_world_point_on_the_canvas() {
        let v = default_view();
        let m = v.view_proj(1.6, 5.0);
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 1000.0));
        let at = project(&m, [1.0, 2.0, 3.0], rect).expect("the target is visible");
        assert!((at.x - 800.0).abs() < 1.0 && (at.y - 500.0).abs() < 1.0, "{at:?}");
    }
}
