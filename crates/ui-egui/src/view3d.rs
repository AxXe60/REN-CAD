//! The 3D view: an orbit camera over the session's point clouds, and 3D drafting on top of them.
//!
//! The drawing canvas is 2D; when the 3D view is on, [`show`] replaces it inside the same rect
//! and handles its own input (drag orbits, middle/shift-drag pans, wheel zooms towards the cursor,
//! double-click fits). The points come from the clouds' existing decimation pyramid, picked by the
//! on-screen density, culled to the ball the camera can see and capped at [`MAX_SCENE_POINTS`], and
//! drawn by [`crate::gpu3d`] — one upload per level, colour, crop or visible-ball change, then one
//! instanced draw call per frame. Orbiting never re-uploads: the ball around the target is the same
//! at every angle.
//!
//! The camera has two projections: [`Projection::Perspective`] with a long lens (35° by default,
//! adjustable), and [`Projection::Ortho`], which removes perspective entirely so walls stay
//! parallel and measuring off the screen is honest. Both frame the same world height at the
//! target, so toggling keeps the view.
//!
//! Drafting ([`Draft3d`]) turns clicks into a 3D polyline: a click snaps to the nearest cloud point
//! on screen (or lands on the view plane through the orbit target), and the finished stroke goes
//! into the session's [`cadcraft_pointcloud::Sketch3d`] through the `3dline` command. The strokes
//! are projected with the same camera and painted over the points, so a wall can be traced in
//! elevation without ever leaving the 3D view.
//!
//! The snap is smart about depth: the 3D pipeline has no depth buffer, so the candidates under the
//! cursor are ranked by their distance to the camera and only the front layer is kept. A click
//! therefore cannot snap through a wall seen through a gap in the near one. Two modes: `Exact` (the
//! cloud point itself) and `Smooth` (the average of the points around it, which cancels the
//! 2-5 mm scan noise a wall is made of).
//!
//! Three constraints make the tracing behave like drafting: the **Z lock** holds the stroke in the
//! horizontal plane of its first vertex, the **axis lock** makes a segment follow the cardinal axis
//! it is closest to (hold Shift), and the **measure tool** reports the length, the run in plan and
//! the rise of a segment.

use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use cadcraft_pointcloud::CloudColor;
use cadcraft_pointcloud::{FRONT_LAYER_M, SMOOTH_RADIUS_M, SnapHit, SnapMode, axis_locked, front_layer, nearest, smooth_centroid, z_locked};
use egui::{Pos2, Rect, Sense, pos2, vec2};

use crate::CadApp;
use crate::gpu3d::{MAX_SCENE_POINTS, Point3d, Scene3d, SceneSlot};
use crate::theme::Tokens;

/// How the camera projects the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Projection {
    /// A long-lens perspective: parallel lines still converge, but gently.
    Perspective,
    /// Parallel projection: no fish-eye at all, and equal world lengths stay equal on screen.
    Ortho,
}

impl Projection {
    pub fn name(self) -> &'static str {
        match self {
            Self::Perspective => "persp",
            Self::Ortho => "ortho",
        }
    }
}

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
    /// Vertical field of view, radians (the "lens": small is telephoto, large is wide).
    pub fov_y: f64,
    pub projection: Projection,
    /// Point square side, points.
    pub point_size: f32,
    /// The front wall cut: the points closer to the camera than this fraction of the distance to
    /// the target are dropped, so the camera can look into a room without cropping the walls out
    /// of the cloud. 0 = off.
    pub front_cut: f32,
    /// How strong the depth cue is (0..1): the farthest points fade into the background and get
    /// smaller. 1 fades them by 45%.
    pub depth_cue: f32,
    /// Fit the camera to the clouds on the next frame.
    pub fitted: bool,
    /// The stroke being drawn, if 3D drafting is on.
    pub draft: Draft3d,
    /// The scene last handed to the GPU, and how many points it holds.
    pub key: Option<u64>,
    pub drawn: usize,
    slot: SceneSlot,
}

/// What a click in the 3D view does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DraftTool {
    /// Place the vertices of a polyline.
    #[default]
    Draw,
    /// Measure between two points.
    Measure,
}

impl DraftTool {
    pub fn name(self) -> &'static str {
        match self {
            Self::Draw => "draw",
            Self::Measure => "measure",
        }
    }
}

/// A measurement taken in the 3D view: the two points and what they measure.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measured {
    pub a: [f64; 3],
    pub b: [f64; 3],
    pub measure: cadcraft_pointcloud::Measure,
}

/// The 3D polyline being drawn in the view.
#[derive(Debug)]
pub struct Draft3d {
    pub active: bool,
    /// Whether the clicks place vertices or measure a segment.
    pub tool: DraftTool,
    /// Vertices placed so far in the current stroke.
    pub points: Vec<[f64; 3]>,
    /// Where the next click would land.
    pub hover: Option<[f64; 3]>,
    /// Whether [`Self::hover`] is a cloud point (snapped) rather than the view plane.
    pub snapped: bool,
    /// Hold the stroke in the horizontal plane of its first vertex.
    pub z_lock: bool,
    /// Make every segment follow the cardinal axis it is closest to (Shift does it for one move).
    pub ortho: bool,
    /// The layer the finished stroke is written on.
    pub layer: String,
    /// The last measurement, kept on screen until the next one.
    pub measure: Option<Measured>,
}

impl Draft3d {
    /// Drop the vertices and the preview, staying in drafting mode for the next stroke.
    pub fn cancel(&mut self) {
        self.points.clear();
        self.hover = None;
        self.snapped = false;
    }
}

impl Default for Draft3d {
    fn default() -> Self {
        Self {
            active: false,
            tool: DraftTool::Draw,
            points: Vec::new(),
            hover: None,
            snapped: false,
            z_lock: false,
            ortho: false,
            layer: cadcraft_pointcloud::DEFAULT_LAYER.to_string(),
            measure: None,
        }
    }
}

impl Default for View3d {
    fn default() -> Self {
        Self {
            active: false,
            target: [0.0; 3],
            yaw: -2.2,
            pitch: 0.55,
            dist: 30.0,
            // 35° vertical: a normal lens, not the 50° wide angle that made near walls bend.
            fov_y: 35.0_f64.to_radians(),
            projection: Projection::Perspective,
            point_size: 1.5,
            front_cut: 0.0,
            depth_cue: 1.0,
            fitted: false,
            draft: Draft3d::default(),
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

    /// The unit vector the camera looks along.
    pub fn forward(&self) -> [f64; 3] {
        self.basis().0
    }

    /// The distance in front of the camera below which points are dropped (the front wall cut).
    pub fn near_cut(&self) -> f64 {
        (f64::from(self.front_cut) * self.dist).max(0.0)
    }

    /// How far behind the target the depth cue reaches its full strength.
    pub fn cue_span(&self) -> f64 {
        (self.dist * 2.0).max(1e-3)
    }

    /// The depth of the orbit target, where the depth cue starts: the points in front of it keep
    /// their full colour and size.
    pub fn target_depth(&self) -> f64 {
        self.dist
    }

    /// World height of the view volume at the target plane: the pan step and the pixels-per-metre
    /// scale, the same in both projections so switching keeps the framing.
    pub fn view_height(&self) -> f64 {
        2.0 * self.dist * (self.fov_y * 0.5).tan()
    }

    /// Column-major world → clip matrix for a canvas of `aspect` width/height.
    pub fn view_proj(&self, aspect: f64, radius: f64) -> [f32; 16] {
        let eye = self.eye();
        let view = look_at(eye, self.target, [0.0, 0.0, 1.0]);
        let aspect = aspect.max(1e-3) as f32;
        match self.projection {
            Projection::Perspective => {
                let near = (self.dist * 0.01).max(1e-3);
                // No depth test, so the far plane only has to be beyond the scene (the visible
                // ball reaches about four eye distances).
                let far = (self.dist * 6.0 + radius * 4.0).max(1.0);
                mul(&perspective(self.fov_y as f32, aspect, near as f32, far as f32), &view)
            }
            Projection::Ortho => {
                let half_h = (self.dist * (self.fov_y * 0.5).tan()).max(1e-6);
                let half_w = half_h * f64::from(aspect);
                // Near at the eye plane: anything behind the camera is clipped (there is no
                // perspective divide to sort it out), and the far side of the visible ball fits.
                let far = (self.dist * 5.0).max(radius * 2.0).max(1.0);
                mul(&ortho(half_w as f32, half_h as f32, far as f32), &view)
            }
        }
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

    /// A standard view, keeping the target and distance.
    pub fn preset(&mut self, yaw: f64, pitch: f64) {
        self.yaw = yaw;
        self.pitch = pitch.clamp(-1.5, 1.5);
    }

    /// Look from above: X right, Y up.
    pub fn top(&mut self) {
        self.preset(-std::f64::consts::FRAC_PI_2, 1.5);
    }

    /// Look along +Y: X right, Z up.
    pub fn front(&mut self) {
        self.preset(-std::f64::consts::FRAC_PI_2, 0.0);
    }

    /// The default three-quarter view.
    pub fn iso(&mut self) {
        self.preset(-2.2, 0.55);
    }

    /// The eye ray through a screen point: from the eye in perspective, parallel from the view
    /// plane in ortho.
    fn ray(&self, screen: Pos2, rect: Rect) -> Option<([f64; 3], [f64; 3])> {
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return None;
        }
        let nx = (f64::from(screen.x - rect.left()) / f64::from(rect.width())) * 2.0 - 1.0;
        let ny = 1.0 - (f64::from(screen.y - rect.top()) / f64::from(rect.height())) * 2.0;
        let aspect = f64::from(rect.width()) / f64::from(rect.height());
        let (f, r, u) = self.basis();
        let tan = (self.fov_y * 0.5).tan();
        match self.projection {
            Projection::Perspective => {
                let d = normalize([
                    f[0] + r[0] * nx * tan * aspect + u[0] * ny * tan,
                    f[1] + r[1] * nx * tan * aspect + u[1] * ny * tan,
                    f[2] + r[2] * nx * tan * aspect + u[2] * ny * tan,
                ]);
                Some((self.eye(), d))
            }
            Projection::Ortho => {
                let h = self.dist * tan;
                let eye = self.eye();
                let o = [
                    eye[0] + r[0] * nx * h * aspect + u[0] * ny * h,
                    eye[1] + r[1] * nx * h * aspect + u[1] * ny * h,
                    eye[2] + r[2] * nx * h * aspect + u[2] * ny * h,
                ];
                Some((o, f))
            }
        }
    }

    /// The world point under `screen` on the plane through the target facing the camera.
    pub fn plane_point(&self, screen: Pos2, rect: Rect) -> Option<[f64; 3]> {
        let (origin, dir) = self.ray(screen, rect)?;
        let (f, _, _) = self.basis();
        let denom = dot(dir, f);
        if denom.abs() < 1e-9 {
            return None;
        }
        let t = dot(sub(self.target, origin), f) / denom;
        if !t.is_finite() || t <= 0.0 {
            return None;
        }
        Some([origin[0] + dir[0] * t, origin[1] + dir[1] * t, origin[2] + dir[2] * t])
    }

    /// Zoom by `wheel` points, keeping the world point under the cursor where it is.
    pub fn zoom_at(&mut self, wheel: f64, at: Option<Pos2>, rect: Rect) {
        if wheel == 0.0 {
            return;
        }
        let factor = (-wheel * 0.002).exp();
        let before = at.and_then(|p| self.plane_point(p, rect));
        self.dist = (self.dist * factor).clamp(0.02, 1e7);
        if let (Some(before), Some(_), Some(after)) = (before, at, at.and_then(|p| self.plane_point(p, rect))) {
            for i in 0..3 {
                let shift = before[i] - after[i];
                if shift.is_finite() {
                    self.target[i] += shift;
                }
            }
        }
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

/// The ball around the orbit target that the camera can see at every orbit angle: the screen
/// corners at the target plane are about one `dist` away, and the third `dist` of margin keeps the
/// walls just behind the target (and a quarter of a pan step) inside it.
///
/// It is deliberately a sphere, not the real frustum: orbiting does not change it, so a drag costs
/// nothing, while zooming in shrinks it to the part of the scan actually on screen.
fn view_radius(dist: f64) -> f64 {
    (dist * 3.0).max(0.1)
}

/// Points of `level` inside the visible ball.
fn visible_points(c: &cadcraft_pointcloud::Cloud, level: &cadcraft_pointcloud::CloudLevel, center: [f64; 3], radius: f64) -> usize {
    let r2 = radius * radius;
    level
        .pts
        .iter()
        .filter(|v| {
            let x = f64::from(v.x) + c.origin[0] - center[0];
            let y = f64::from(v.y) + c.origin[1] - center[1];
            let z = f64::from(v.z) - center[2];
            x * x + y * y + z * z <= r2
        })
        .count()
}

/// The level each visible cloud contributes at this density, coarsened until the points actually
/// on screen fit the budget. Zoomed in on one wall the ball holds a small part of the scan, so the
/// full-resolution level is affordable exactly when the detail would be visible.
fn pick_levels(
    clouds: &[cadcraft_pointcloud::Cloud],
    px_per_m: f64,
    center: [f64; 3],
    radius: f64,
) -> Vec<(&cadcraft_pointcloud::Cloud, &cadcraft_pointcloud::CloudLevel)> {
    let mut picks = Vec::new();
    let mut budget = MAX_SCENE_POINTS;
    for c in clouds.iter().filter(|c| c.visible) {
        let chosen = c.level_for(px_per_m);
        let mut i = c.levels.iter().position(|l| std::ptr::eq(l, chosen)).unwrap_or(0);
        let mut n = visible_points(c, &c.levels[i], center, radius);
        while n > budget && i + 1 < c.levels.len() {
            i += 1;
            n = visible_points(c, &c.levels[i], center, radius);
        }
        budget = budget.saturating_sub(n);
        picks.push((c, &c.levels[i]));
    }
    picks
}

/// What the GPU buffer was built from: names, colour modes, level cells, the crop and the visible
/// ball. Orbiting leaves every input alone, so a drag never re-uploads; zooming and panning enter
/// in coarse steps (a quarter of the ball, half of an e-fold of its radius).
fn scene_key(picks: &[(&cadcraft_pointcloud::Cloud, &cadcraft_pointcloud::CloudLevel)], center: [f64; 3], radius: f64) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (c, level) in picks {
        c.name.hash(&mut hasher);
        c.color.name().hash(&mut hasher);
        c.ground_only.hash(&mut hasher);
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
    let r = radius.max(1e-6);
    let step = r * 0.25;
    for v in center {
        ((v / step).round() as i64).hash(&mut hasher);
    }
    ((r.ln() * 2.0).round() as i64).hash(&mut hasher);
    hasher.finish()
}

/// Recolour the picked levels and pack the visible ones for upload.
fn build_scene(picks: &[(&cadcraft_pointcloud::Cloud, &cadcraft_pointcloud::CloudLevel)], center: [f64; 3], radius: f64) -> Scene3d {
    let total: usize = picks.iter().map(|(_, level)| level.pts.len()).sum();
    let r2 = radius * radius;
    let mut scene = Scene3d { points: Vec::with_capacity(total.min(MAX_SCENE_POINTS)) };
    for (c, level) in picks {
        for (i, v) in level.pts.iter().enumerate() {
            // With the ground filter on, only the points it found are drawn.
            if c.ground_only && level.classes.get(i).is_none_or(|k| *k != cadcraft_pointcloud::CLASS_GROUND) {
                continue;
            }
            let (x, y, z) = (f64::from(v.x) + c.origin[0], f64::from(v.y) + c.origin[1], f64::from(v.z));
            let (dx, dy, dz) = (x - center[0], y - center[1], z - center[2]);
            if dx * dx + dy * dy + dz * dz > r2 {
                continue;
            }
            if c.clip.is_some_and(|clip| !clip.contains(x, y, z)) {
                continue;
            }
            let [r, g, b] = c.color_of(v);
            scene.points.push(Point3d { pos: [x as f32, y as f32, z as f32], color: [r, g, b, 255] });
        }
    }
    scene
}

/// A screen projection cached for a whole frame: the points, the sketch and snapping all use it.
struct Screen {
    vp: [f32; 16],
    rect: Rect,
    eye: [f64; 3],
    forward: [f64; 3],
}

impl Screen {
    fn new(view: &View3d, vp: [f32; 16], rect: Rect) -> Self {
        let (forward, _, _) = view.basis();
        Self { vp, rect, eye: view.eye(), forward }
    }

    /// Project a world point, rejecting what the camera cannot see (behind it, or clipped by the
    /// near/far planes). Outside the rect is kept: egui clips the sketch at the view edge.
    fn project(&self, p: [f64; 3]) -> Option<Pos2> {
        let relative = sub(p, self.eye);
        if dot(relative, self.forward) <= 0.0 {
            return None;
        }
        let c = transform(&self.vp, [p[0] as f32, p[1] as f32, p[2] as f32]);
        if c[3] <= 1e-6 || !(0.0..=1.0).contains(&(c[2] / c[3])) {
            return None;
        }
        let x = c[0] / c[3];
        let y = c[1] / c[3];
        Some(pos2(self.rect.left() + (x + 1.0) * 0.5 * self.rect.width(), self.rect.top() + (1.0 - y) * 0.5 * self.rect.height()))
    }
}

/// How close to a cloud point, in screen points, a click snaps to it.
const SNAP_PX: f32 = 14.0;

/// How many candidates under the cursor are kept before the front layer is cut out of them.
const MAX_SNAP_HITS: usize = 20_000;

/// The cloud point a click snaps to, searched on a sample of the scene so a hover costs a
/// millisecond: the scan is dense enough that a 1-in-12 sample still lands within a millimetre.
///
/// The candidates are ranked by their distance to the camera and only the **front layer** is kept —
/// the shallowest ones plus everything within [`FRONT_LAYER_M`] behind them. That is what keeps a
/// click from snapping through a far wall seen through a gap in the near one, without a depth
/// buffer in the pipeline.
///
/// In `Smooth` mode the point is the average of the cloud points around the winner (a 3 cm
/// sphere), which cancels the 2-5 mm scan noise; in `Exact` mode it is the cloud point itself.
fn snap_to_cloud(slot: &SceneSlot, screen: &Screen, at: Pos2, mode: SnapMode) -> Option<[f64; 3]> {
    let guard = slot.lock().ok()?;
    let scene = guard.as_ref()?;
    let n = scene.points.len();
    if n == 0 {
        return None;
    }
    let step = (n / 250_000).max(1);
    let mut hits: Vec<SnapHit> = Vec::new();
    for p in scene.points.iter().step_by(step) {
        let world = [f64::from(p.pos[0]), f64::from(p.pos[1]), f64::from(p.pos[2])];
        let Some(q) = screen.project(world) else { continue };
        let dx = q.x - at.x;
        let dy = q.y - at.y;
        if dx * dx + dy * dy > SNAP_PX * SNAP_PX {
            continue;
        }
        let depth = dot(sub(world, screen.eye), screen.forward);
        if hits.len() >= MAX_SNAP_HITS {
            // A candidate deeper than the current front can never come back into it, so it goes.
            let limit = hits.iter().map(|h| h.depth).fold(f64::INFINITY, f64::min) + FRONT_LAYER_M;
            hits.retain(|h| h.depth <= limit);
            if hits.len() >= MAX_SNAP_HITS {
                break;
            }
        }
        hits.push(SnapHit { point: world, depth, screen: [q.x, q.y] });
    }
    let front = front_layer(&hits, FRONT_LAYER_M);
    let winner = nearest(&front, [at.x, at.y])?;
    match mode {
        SnapMode::Exact => Some(winner.point),
        SnapMode::Smooth => Some(smooth_centroid(
            winner.point,
            scene.points.iter().step_by(step).map(|p| [f64::from(p.pos[0]), f64::from(p.pos[1]), f64::from(p.pos[2])]),
            SMOOTH_RADIUS_M,
        )),
    }
}

/// Where a click at `at` places a vertex: the nearest cloud point within [`SNAP_PX`], else the
/// view plane through the orbit target.
fn place_point(app: &CadApp, screen: &Screen, at: Pos2) -> Option<([f64; 3], bool)> {
    if let Some(p) = snap_to_cloud(&app.view3d.slot, screen, at, app.session.snap3d) {
        return Some((p, true));
    }
    app.view3d.plane_point(at, screen.rect).map(|p| (p, false))
}

/// The drawing constraints: the Z lock holds the stroke in the plane of its first vertex, and the
/// axis lock makes the segment follow the cardinal axis it is closest to (`Shift` does it for one
/// move).
fn constrain(draft: &Draft3d, p: [f64; 3], shift: bool) -> [f64; 3] {
    let mut out = z_locked(p, draft.points.first().copied(), draft.z_lock);
    if (shift || draft.ortho)
        && let Some(last) = draft.points.last().copied()
    {
        out = axis_locked(out, last);
    }
    out
}

/// Commit the stroke being drawn through `3dline` (the same command a script would call). A closed
/// stroke joins its last vertex back to its first.
pub fn finish_draft(app: &mut CadApp, closed: bool) {
    let points = std::mem::take(&mut app.view3d.draft.points);
    let layer = app.view3d.draft.layer.clone();
    app.view3d.draft.hover = None;
    app.view3d.draft.snapped = false;
    if points.is_empty() {
        return;
    }
    if let Err(error) = app.run("3dline", serde_json::json!({ "points": points, "layer": layer, "closed": closed })) {
        app.session.echo(error.clone());
        app.set_status(error);
    }
}

/// Drop the stroke being drawn and leave drafting mode.
pub fn cancel_draft(app: &mut CadApp) {
    app.view3d.draft.cancel();
    app.view3d.draft.active = false;
}

/// Measure a segment through the `3dmeasure` command, so the number on screen is the number a
/// script gets.
fn measure_between(app: &mut CadApp, a: [f64; 3], b: [f64; 3]) -> Measured {
    let mut measure = cadcraft_pointcloud::measure(a, b);
    if let Ok(value) = app.run("3dmeasure", serde_json::json!({ "p1": a, "p2": b }))
        && let Some(length) = value.get("dist_3d").and_then(serde_json::Value::as_f64)
        && let Some(flat) = value.get("dist_2d_xy").and_then(serde_json::Value::as_f64)
    {
        measure.length = length;
        measure.flat = flat;
    }
    Measured { a, b, measure }
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

    let height = f64::from(rect.height()).max(1.0);
    let aspect = f64::from(rect.width()) / height;
    let radius = cloud_bounds(app).map_or(10.0, |(min, max)| {
        let d = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
        0.5 * (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(0.1)
    });

    // ---------- input ----------
    let resp = ui.interact(rect, ui.id().with("view3d"), Sense::click_and_drag());
    let (shift, wheel, middle, pointer_delta, pointer, inside) = ui.input(|i| {
        let pointer = i.pointer.hover_pos();
        (i.modifiers.shift, i.smooth_scroll_delta.y, i.pointer.middle_down(), i.pointer.delta(), pointer, pointer.is_some_and(|p| rect.contains(p)))
    });
    if inside && wheel != 0.0 {
        app.view3d.zoom_at(f64::from(wheel), pointer, rect);
    }
    // Middle-drag panning is read from the pointer (egui's drag sense follows the primary button).
    let drag = if inside && middle && pointer_delta != egui::Vec2::ZERO {
        Some((pointer_delta, false))
    } else if resp.dragged() {
        Some((resp.drag_delta(), shift && resp.dragged_by(egui::PointerButton::Primary)))
    } else if resp.dragged_by(egui::PointerButton::Secondary) && pointer_delta != egui::Vec2::ZERO && !app.view3d.draft.active {
        // Right-drag pans too (a 3D view habit); in drafting, right-click ends the stroke.
        Some((pointer_delta, true))
    } else {
        None
    };
    if let Some((d, pan)) = drag {
        if pan {
            let k = app.view3d.view_height() / height;
            let (_f, right, up) = app.view3d.basis();
            for i in 0..3 {
                app.view3d.target[i] += (-right[i] * f64::from(d.x) + up[i] * f64::from(d.y)) * k;
            }
        } else {
            app.view3d.yaw -= f64::from(d.x) * 0.008;
            app.view3d.pitch = (app.view3d.pitch - f64::from(d.y) * 0.008).clamp(-1.5, 1.5);
        }
    }

    // The matrix the clicks of this frame use: after the camera input, before the scene rebuild.
    let vp = app.view3d.view_proj(aspect, radius);

    // ---------- drafting ----------
    let mut finished = false;
    let mut cancelled = false;
    let mut close_stroke = false;
    if app.view3d.draft.active {
        let screen = Screen::new(&app.view3d, vp, rect);
        let at = pointer.filter(|p| rect.contains(*p));
        let placed = at.and_then(|p| place_point(app, &screen, p)).map(|(p, s)| (constrain(&app.view3d.draft, p, shift), s));
        app.view3d.draft.hover = placed.map(|(p, _)| p);
        app.view3d.draft.snapped = placed.is_some_and(|(_, s)| s);

        // A double-click ends the stroke (its first click already placed the last vertex); a
        // right-click ends it without adding one.
        let double = resp.double_clicked();
        let secondary = resp.secondary_clicked();
        let measuring = app.view3d.draft.tool == DraftTool::Measure;
        finished = (double || secondary) && !measuring;
        let add = resp.clicked() && !resp.dragged() && !double && !secondary;
        if add && let Some((p, _)) = placed {
            app.view3d.draft.points.push(p);
            if measuring && app.view3d.draft.points.len() == 2 {
                let (a, b) = (app.view3d.draft.points[0], app.view3d.draft.points[1]);
                let measured = measure_between(app, a, b);
                app.view3d.draft.measure = Some(measured);
                app.view3d.draft.points.clear();
            }
        }
        if (double || secondary) && measuring {
            // A right-click drops the pending measurement.
            app.view3d.draft.points.clear();
        }
        // Z holds the plane, C closes the stroke. They are consumed so they never reach the
        // command line as text.
        if inside {
            let (z, c) = ui.input_mut(|i| {
                let z = i.consume_key(egui::Modifiers::NONE, egui::Key::Z);
                let c = i.consume_key(egui::Modifiers::NONE, egui::Key::C);
                if z || c {
                    i.events.retain(|e| !matches!(e, egui::Event::Text(t) if t.eq_ignore_ascii_case("z") || t.eq_ignore_ascii_case("c")));
                }
                (z, c)
            });
            if z {
                app.view3d.draft.z_lock = !app.view3d.draft.z_lock;
            }
            close_stroke = c && !measuring;
        }
        cancelled = inside && ui.input(|i| i.key_pressed(egui::Key::Escape));
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    if cancelled {
        app.view3d.draft.cancel();
    }
    if close_stroke {
        finish_draft(app, true);
    } else if finished {
        finish_draft(app, false);
    }
    if resp.double_clicked() && !app.view3d.draft.active {
        let bounds = cloud_bounds(app);
        app.view3d.fit(bounds);
    }

    // ---------- scene ----------
    let px_per_m = height / app.view3d.view_height().max(1e-9);
    // Only the ball around the orbit target is uploaded; zooming in shrinks it, so the finer
    // levels become affordable where their detail shows.
    let center = app.view3d.target;
    let radius = view_radius(app.view3d.dist);
    let picks = pick_levels(&app.session.clouds, px_per_m, center, radius);
    let key = scene_key(&picks, center, radius);
    if app.view3d.key != Some(key) {
        let started = std::time::Instant::now();
        let scene = build_scene(&picks, center, radius);
        let drawn = scene.points.len();
        if let Ok(mut slot) = app.view3d.slot.lock() {
            *slot = Some(scene);
        }
        app.view3d.key = Some(key);
        app.view3d.drawn = drawn;
        log::info!("cloud 3d scene: {drawn} visible points in {:.1} ms", started.elapsed().as_secs_f64() * 1000.0);
    }

    if let Some(target) = app.canvas.gpu {
        let bg = t.canvas;
        let eye = app.view3d.eye();
        let forward = app.view3d.forward();
        painter.add(egui_wgpu::Callback::new_paint_callback(
            rect,
            crate::gpu3d::Cloud3dCallback {
                key,
                slot: app.view3d.scene_slot(),
                view_proj: vp,
                viewport: [rect.width(), rect.height()],
                point_size: app.view3d.point_size,
                eye: [eye[0] as f32, eye[1] as f32, eye[2] as f32],
                forward: [forward[0] as f32, forward[1] as f32, forward[2] as f32],
                near_cut: app.view3d.near_cut() as f32,
                cue_span: app.view3d.cue_span() as f32,
                target_depth: app.view3d.target_depth() as f32,
                cue: app.view3d.depth_cue,
                bg: [f32::from(bg.r()) / 255.0, f32::from(bg.g()) / 255.0, f32::from(bg.b()) / 255.0, 1.0],
                stencil: target.stencil,
            },
        ));
    } else {
        // No wgpu render state (wasm without a GPU): project on the CPU, sampled.
        draw_cpu_fallback(&painter, rect, &vp, &app.view3d, &app.session.clouds, px_per_m, center, radius);
    }

    // ---------- sketch ----------
    draw_sketch(&painter, &app.view3d, &vp, rect, &app.session.sketch);

    // ---------- HUD ----------
    let clouds = app.session.clouds.iter().filter(|c| c.visible).count();
    let mut info = format!(
        "3D · {} · {clouds} cloud{}, {} points on screen\nDrag: orbit · Shift or middle drag: pan · Wheel: zoom at cursor · Double-click: fit",
        app.view3d.projection.name(),
        if clouds == 1 { "" } else { "s" },
        app.view3d.drawn
    );
    if app.view3d.draft.active {
        let draft = &app.view3d.draft;
        if draft.tool == DraftTool::Measure {
            info.push_str("\nMeasure 3D: click two points (they snap to the cloud) · right-click clears · Esc leaves");
        } else {
            info.push_str("\nDraw 3D: click places a vertex (snaps to the cloud) · right-click or double-click ends · Esc cancels");
        }
        info.push_str(&format!("\nSnap: {} · Z: plane lock {}", app.session.snap3d.name(), if draft.z_lock { "on" } else { "off" }));
        if draft.ortho {
            info.push_str(" · Ortho on (Shift for one move)");
        }
        if let Some(m) = draft.measure {
            info.push_str(&format!("\n{}", cadcraft_pointcloud::readout(&m.measure)));
        }
    } else if !app.session.sketch.is_empty() {
        info.push_str(&format!("\n3D sketch: {} strokes, {} segments", app.session.sketch.count(), app.session.sketch.segments()));
    }
    painter.text(rect.left_top() + vec2(10.0, 8.0), egui::Align2::LEFT_TOP, info, egui::FontId::proportional(12.0), t.text_dim);
}

/// Paint the committed strokes and the stroke being drawn over the points.
fn draw_sketch(painter: &egui::Painter, view: &View3d, vp: &[f32; 16], rect: Rect, sketch: &cadcraft_pointcloud::Sketch3d) {
    if sketch.is_empty() && !view.draft.active {
        return;
    }
    let screen = Screen::new(view, *vp, rect);
    let color = egui::Color32::from_rgb(0xF2, 0x7A, 0x1A);
    for (a, b) in sketch.iter_segments() {
        if let (Some(pa), Some(pb)) = (screen.project(a), screen.project(b)) {
            painter.line_segment([pa, pb], egui::Stroke::new(2.0, color));
        }
    }
    for stroke in sketch.strokes() {
        for p in &stroke.points {
            if let Some(q) = screen.project(*p) {
                painter.circle_filled(q, 2.5, color);
            }
        }
    }

    let draft = &view.draft;
    if !draft.active {
        return;
    }
    for p in &draft.points {
        if let Some(q) = screen.project(*p) {
            painter.circle_filled(q, 3.5, color);
        }
    }
    if let (Some(last), Some(hover)) = (draft.points.last(), draft.hover)
        && let (Some(a), Some(b)) = (screen.project(*last), screen.project(hover))
    {
        painter.line_segment([a, b], egui::Stroke::new(1.5, color.gamma_multiply(0.75)));
        // The live numbers of the segment being drawn: its length, its run and its rise.
        let m = cadcraft_pointcloud::measure(*last, hover);
        painter.text(b + vec2(10.0, -10.0), egui::Align2::LEFT_BOTTOM, cadcraft_pointcloud::readout(&m), egui::FontId::monospace(12.0), color);
    }
    if let Some(hover) = draft.hover
        && let Some(q) = screen.project(hover)
    {
        let stroke = egui::Stroke::new(1.5, if draft.snapped { egui::Color32::from_rgb(0x6C, 0xE0, 0x7A) } else { color });
        painter.circle_stroke(q, 6.0, stroke);
        if draft.snapped {
            painter.circle_filled(q, 2.0, stroke.color);
        }
    }
    // The measurement that was taken: the segment and its numbers.
    if let Some(m) = draft.measure
        && let (Some(a), Some(b)) = (screen.project(m.a), screen.project(m.b))
    {
        let blue = egui::Color32::from_rgb(0x4A, 0xC8, 0xFF);
        painter.line_segment([a, b], egui::Stroke::new(2.0, blue));
        painter.circle_stroke(a, 4.0, egui::Stroke::new(1.5, blue));
        painter.circle_stroke(b, 4.0, egui::Stroke::new(1.5, blue));
        let mid = pos2((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
        painter.text(
            mid + vec2(10.0, -10.0),
            egui::Align2::LEFT_BOTTOM,
            cadcraft_pointcloud::readout(&m.measure),
            egui::FontId::monospace(12.0),
            blue,
        );
    }
}

fn draw_cpu_fallback(
    painter: &egui::Painter,
    rect: Rect,
    vp: &[f32; 16],
    view: &View3d,
    clouds: &[cadcraft_pointcloud::Cloud],
    px_per_m: f64,
    center: [f64; 3],
    radius: f64,
) {
    let mut shapes = Vec::new();
    let r2 = radius * radius;
    let screen = Screen::new(view, *vp, rect);
    for c in clouds.iter().filter(|c| c.visible) {
        let level = c.level_for(px_per_m);
        let step = (level.pts.len() / 50_000).max(1);
        for v in level.pts.iter().step_by(step) {
            let (x, y, z) = (f64::from(v.x) + c.origin[0], f64::from(v.y) + c.origin[1], f64::from(v.z));
            let (dx, dy, dz) = (x - center[0], y - center[1], z - center[2]);
            if dx * dx + dy * dy + dz * dz > r2 {
                continue;
            }
            if c.clip.is_some_and(|clip| !clip.contains(x, y, z)) {
                continue;
            }
            if let Some(at) = screen.project([x, y, z]) {
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

#[cfg(test)]
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

/// Right-handed orthographic, WebGPU clip depth 0..1, column-major. Near is the eye plane.
fn ortho(half_w: f32, half_h: f32, far: f32) -> [f32; 16] {
    let (w, h) = (half_w.max(1e-6), half_h.max(1e-6));
    let f = far.max(1e-6);
    [1.0 / w, 0.0, 0.0, 0.0, 0.0, 1.0 / h, 0.0, 0.0, 0.0, 0.0, -1.0 / f, 0.0, 0.0, 0.0, 0.0, 1.0]
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
        let picks = pick_levels(&[], 50.0, [0.0; 3], 10.0);
        let scene = build_scene(&picks, [0.0; 3], 10.0);
        assert_eq!(scene.points.len(), 0);
        assert_eq!(scene_key(&picks, [0.0; 3], 10.0), scene_key(&[], [0.0; 3], 10.0));
    }

    /// A 10 m ball over a 100 m line: only the points inside it are counted and uploaded, which is
    /// what lets a zoomed-in view afford the full-resolution level.
    #[test]
    fn only_the_ball_around_the_target_is_counted_and_uploaded() {
        let pts: Vec<_> =
            (0..1000).map(|i| cadcraft_pointcloud::Pt { x: f64::from(i) * 0.1, y: 0.0, z: 1.0, rgb: None, intensity: 0, class: 1 }).collect();
        let mut b = cadcraft_pointcloud::Bounds3::EMPTY;
        for p in &pts {
            b.add(p.x, p.y, p.z);
        }
        let slice = cadcraft_pointcloud::Slice { points: pts, total: 1000, in_band: 1000, bounds: b };
        let c = cadcraft_pointcloud::Cloud::from_slice("t".into(), String::new(), 0.0, 2.0, None, slice, 1_000_000, CloudColor::Rgb);
        let (center, radius) = ([10.0, 0.0, 1.0], 5.0);
        let n = visible_points(&c, &c.levels[0], center, radius);
        assert!((90..=110).contains(&n), "a 10 m ball of a 0.1 m line: {n}");
        let clouds = [c];
        let picks = pick_levels(&clouds, 4000.0, center, radius);
        assert_eq!(build_scene(&picks, center, radius).points.len(), n);
    }

    /// Orbiting changes no key input; zooming and panning do, in coarse steps.
    #[test]
    fn the_scene_key_follows_the_visible_ball_not_the_camera_angle() {
        let k = scene_key(&[], [0.0; 3], 10.0);
        assert_eq!(k, scene_key(&[], [0.0; 3], 10.0), "a drag re-uses the buffer");
        assert_ne!(k, scene_key(&[], [5.0, 0.0, 0.0], 10.0), "panning far enough rebuilds");
        assert_ne!(k, scene_key(&[], [0.0; 3], 4.0), "zooming rebuilds");
    }

    #[test]
    fn projection_puts_a_world_point_on_the_canvas() {
        let v = default_view();
        let m = v.view_proj(1.6, 5.0);
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 1000.0));
        let at = project(&m, [1.0, 2.0, 3.0], rect).expect("the target is visible");
        assert!((at.x - 800.0).abs() < 1.0 && (at.y - 500.0).abs() < 1.0, "{at:?}");
    }

    /// Ortho has no perspective divide, so the same world offset is the same on screen at any
    /// depth. That is the property that kills the fish-eye look.
    #[test]
    fn ortho_keeps_the_scale_with_depth() {
        let mut v = default_view();
        v.projection = Projection::Ortho;
        let m = v.view_proj(1.0, 5.0);
        let near = project(&m, [1.0, 2.0 + 1.0, 3.0], Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 1000.0)));
        let far = project(&m, [1.0 - 5.0, 2.0 + 1.0, 3.0], Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 1000.0)));
        let (near, far) = (near.unwrap_or(pos2(0.0, 0.0)), far.unwrap_or(pos2(0.0, 0.0)));
        assert!((near.x - far.x).abs() < 0.01, "same offset, same pixel column: {near:?} vs {far:?}");
    }

    /// Zooming at the cursor keeps the world point under it fixed, so the view grows towards what
    /// the user points at instead of the middle of the screen.
    #[test]
    fn zoom_anchors_the_point_under_the_cursor() {
        let mut v = default_view();
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0));
        let at = pos2(820.0, 260.0);
        let before = v.plane_point(at, rect).expect("the cursor ray crosses the target plane");
        v.zoom_at(120.0, Some(at), rect);
        let after = v.plane_point(at, rect).expect("still crosses after zooming");
        for i in 0..3 {
            assert!((before[i] - after[i]).abs() < 1e-6, "axis {i}: {before:?} vs {after:?}");
        }
        assert!(v.dist < 10.0, "a positive wheel zooms in: {}", v.dist);
    }

    #[test]
    fn presets_look_where_their_names_say() {
        let mut v = default_view();
        v.top();
        assert!(v.pitch > 1.4, "top looks down: {}", v.pitch);
        let (f, _, _) = v.basis();
        assert!(f[2] < -0.99, "top looks along -Z: {f:?}");
        v.front();
        assert!(v.pitch.abs() < 1e-9);
        let (f, r, u) = v.basis();
        assert!((f[1] - 1.0).abs() < 1e-9 && r[0] > 0.99 && u[2] > 0.99, "front: +Y with X right and Z up");
        v.iso();
        assert!((v.yaw + 2.2).abs() < 1e-9 && (v.pitch - 0.55).abs() < 1e-9);
    }

    /// The Z lock holds the stroke in the plane of its first vertex; the axis lock makes the
    /// segment leave the last vertex along one cardinal axis.
    #[test]
    fn the_constraints_shape_the_points() {
        let mut draft = Draft3d::default();
        draft.points.push([1.0, 2.0, 3.0]);
        draft.z_lock = true;
        assert_eq!(constrain(&draft, [5.0, 6.0, 9.0], false), [5.0, 6.0, 3.0], "the plane holds");
        draft.z_lock = false;
        assert_eq!(constrain(&draft, [5.0, 6.0, 9.0], false), [5.0, 6.0, 9.0], "off: the point is untouched");
        assert_eq!(constrain(&draft, [5.0, 2.1, 3.05], true), [5.0, 2.0, 3.0], "Shift: one axis only");
        draft.ortho = true;
        assert_eq!(constrain(&draft, [5.0, 2.1, 3.05], false), [5.0, 2.0, 3.0], "the toggle does the same");
    }

    #[test]
    fn a_new_draft_writes_on_the_default_layer() {
        assert_eq!(Draft3d::default().layer, cadcraft_pointcloud::DEFAULT_LAYER);
        assert_eq!(Draft3d::default().tool, DraftTool::Draw);
    }

    /// The front wall cut and the depth cue are distances, so they follow the camera.
    #[test]
    fn the_cut_and_the_cue_follow_the_camera() {
        let mut v = default_view();
        v.dist = 20.0;
        assert_eq!(v.near_cut(), 0.0, "the cut is off until it is asked for");
        assert!((v.target_depth() - 20.0).abs() < 1e-9, "the cue starts at the target");
        assert!((v.cue_span() - 40.0).abs() < 1e-9, "and reaches its strength two distances behind it");
        v.front_cut = 0.3;
        assert!((v.near_cut() - 6.0).abs() < 1e-5, "a third of the way to the target: {}", v.near_cut());
    }
}
