//! The point cloud's viewport HUD: a floating bar over the 3D view, and the Z band panel with
//! the scan's height histogram.
//!
//! The panel and the bar are thin: they read the session's state and push [`PointCloudUiAction`]s,
//! which the caller turns into commands (`LASCLIP`, `LASOPACITY`, `LASFRAME`) or into view state
//! (`app.view3d`). Nothing here owns state of its own, so the palette, the bar and the command
//! line can never disagree.

use egui::{Align2, Color32, CornerRadius, FontId, Id, Order, Painter, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};

/// The camera's standard views.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewPreset {
    Top,
    Front,
    Right,
    Iso,
}

/// What the HUD asks the app to do. Everything here has a command behind it.
#[derive(Debug, Clone, PartialEq)]
pub enum PointCloudUiAction {
    /// Turn the 3D view on or off.
    Set3DView(bool),
    /// A standard camera view.
    SetViewPreset(ViewPreset),
    /// Perspective or orthographic.
    SetOrtho(bool),
    /// How opaque the cloud is drawn.
    SetOpacity(f32),
    /// Frame the cloud (LASFRAME).
    FitFrame,
    /// Cut the cloud to a Z band.
    SetZClip { z_min: f64, z_max: f64 },
    /// Cut the cloud to everything again.
    ResetZClip,
}

/// The state the HUD draws. It is a copy of the session's, for one frame.
#[derive(Debug, Clone)]
pub struct PointCloudHudState {
    pub is_3d_active: bool,
    pub is_ortho: bool,
    /// How opaque the cloud is drawn (0.05..=1.0).
    pub opacity: f32,
    /// The scan's height range, in metres.
    pub bounds_z_min: f64,
    pub bounds_z_max: f64,
    /// The current cut, in metres.
    pub clip_z_min: f64,
    pub clip_z_max: f64,
    /// Keep the band's thickness while it moves.
    pub lock_slice_thickness: bool,
    /// How far the band moves per step, in metres.
    pub jog_step_m: f64,
    /// The scan's heights in 64 bins.
    pub z_histogram: [u32; 64],
}

impl Default for PointCloudHudState {
    fn default() -> Self {
        Self {
            is_3d_active: false,
            is_ortho: false,
            opacity: 1.0,
            bounds_z_min: 0.0,
            bounds_z_max: 1.0,
            clip_z_min: 0.0,
            clip_z_max: 1.0,
            lock_slice_thickness: false,
            jog_step_m: 0.15,
            z_histogram: [0; 64],
        }
    }
}

/// The floating bar, top centre of the 3D view.
///
/// The bar is anchored to the top of the window, below the menu and the toolbars: the canvas sits
/// under that strip and the bar must not cover the app's own buttons.
pub fn draw_viewport_floating_hud(ctx: &egui::Context, _panel_rect: Rect, state: &mut PointCloudHudState, actions: &mut Vec<PointCloudUiAction>) {
    egui::Area::new(Id::new("rencad_pointcloud_hud")).order(Order::Foreground).anchor(Align2::CENTER_TOP, Vec2::new(0.0, 66.0)).show(ctx, |ui| {
        let frame = egui::Frame::NONE
            .fill(Color32::from_rgba_unmultiplied(24, 26, 31, 230))
            .stroke(Stroke::new(1.0, Color32::from_rgb(58, 64, 76)))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(10, 6));
        frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;

                // 2D / 3D
                let label = if state.is_3d_active { "3D View" } else { "2D Plan" };
                if ui
                    .selectable_label(state.is_3d_active, RichText::new(label).strong())
                    .on_hover_text("Switch between the 2D drawing and the 3D view")
                    .clicked()
                {
                    state.is_3d_active = !state.is_3d_active;
                    actions.push(PointCloudUiAction::Set3DView(state.is_3d_active));
                }
                ui.separator();

                // The standard views
                for (preset, label, tip) in [
                    (ViewPreset::Top, "Top", "Look down: X right, Y up"),
                    (ViewPreset::Front, "Front", "Look along +Y: X right, Z up"),
                    (ViewPreset::Right, "Right", "Look along +X: Y right, Z up"),
                    (ViewPreset::Iso, "Iso", "The three-quarter default"),
                ] {
                    if ui.button(label).on_hover_text(tip).clicked() {
                        actions.push(PointCloudUiAction::SetViewPreset(preset));
                    }
                }

                // Perspective or orthographic
                let proj = if state.is_ortho { "Ortho" } else { "35° Persp" };
                if ui.selectable_label(state.is_ortho, proj).on_hover_text("A long lens, or a true orthographic projection").clicked() {
                    state.is_ortho = !state.is_ortho;
                    actions.push(PointCloudUiAction::SetOrtho(state.is_ortho));
                }
                if ui.button("Fit").on_hover_text("Frame the cloud (LASFRAME)").clicked() {
                    actions.push(PointCloudUiAction::FitFrame);
                }
                ui.separator();

                // The opacity
                draw_eye_icon(ui, state.opacity);
                let mut pct = (state.opacity.clamp(0.05, 1.0) * 100.0).round() as i32;
                let slider = egui::Slider::new(&mut pct, 5..=100).suffix("%").show_value(true);
                if ui
                    .add_sized([110.0, 18.0], slider)
                    .on_hover_text("How opaque the cloud is drawn; below 1 the points are thinned, so the drawing under it shows")
                    .changed()
                {
                    let opacity = (pct as f32 / 100.0).clamp(0.05, 1.0);
                    state.opacity = opacity;
                    actions.push(PointCloudUiAction::SetOpacity(opacity));
                }
            });
        });
    });
}

/// The Z band panel: the scan's height histogram, the two cut sliders and the band jogger.
pub fn draw_z_slice_and_histogram_panel(ui: &mut Ui, state: &mut PointCloudHudState, actions: &mut Vec<PointCloudUiAction>) {
    let span = (state.bounds_z_max - state.bounds_z_min).max(0.001);
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Z Band").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Reset").clicked() {
                    state.clip_z_min = state.bounds_z_min;
                    state.clip_z_max = state.bounds_z_max;
                    state.lock_slice_thickness = false;
                    actions.push(PointCloudUiAction::SetZClip { z_min: state.clip_z_min, z_max: state.clip_z_max });
                }
            });
        });

        // The histogram: 64 bins over the scan's height, with the cut marked.
        let height = 46.0;
        let (hist_rect, hist_resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click_and_drag());
        if ui.is_rect_visible(hist_rect) {
            let painter = ui.painter_at(hist_rect);
            painter.rect_filled(hist_rect, 4.0, Color32::from_rgb(20, 22, 27));
            let max_count = state.z_histogram.iter().copied().max().unwrap_or(1).max(1) as f32;
            let bins = state.z_histogram.len() as f32;
            let bin_w = hist_rect.width() / bins;
            let min_norm = ((state.clip_z_min - state.bounds_z_min) / span).clamp(0.0, 1.0) as f32;
            let max_norm = ((state.clip_z_max - state.bounds_z_min) / span).clamp(0.0, 1.0) as f32;
            for (i, &count) in state.z_histogram.iter().enumerate() {
                let inside = (i as f32 + 0.5) / bins >= min_norm && (i as f32 + 0.5) / bins <= max_norm;
                // The square root keeps the thin bins (a wall) visible next to the floor and the
                // ceiling.
                let ratio = (count as f32 / max_count).sqrt().clamp(0.0, 1.0);
                let bar_h = ratio * (height - 6.0);
                let x0 = hist_rect.left() + i as f32 * bin_w;
                let bar = Rect::from_min_max(Pos2::new(x0 + 0.5, hist_rect.bottom() - bar_h), Pos2::new(x0 + bin_w - 0.5, hist_rect.bottom()));
                let colour = if inside { Color32::from_rgb(76, 162, 245) } else { Color32::from_rgb(52, 57, 68) };
                painter.rect_filled(bar, 1.0, colour);
            }
            let x_min = hist_rect.left() + min_norm * hist_rect.width();
            let x_max = hist_rect.left() + max_norm * hist_rect.width();
            painter.line_segment(
                [Pos2::new(x_min, hist_rect.top()), Pos2::new(x_min, hist_rect.bottom())],
                Stroke::new(2.0, Color32::from_rgb(90, 210, 140)),
            );
            painter.line_segment(
                [Pos2::new(x_max, hist_rect.top()), Pos2::new(x_max, hist_rect.bottom())],
                Stroke::new(2.0, Color32::from_rgb(245, 155, 85)),
            );
        }
        // Dragging the histogram moves the band, when its thickness is locked.
        if (hist_resp.dragged() || hist_resp.clicked())
            && state.lock_slice_thickness
            && hist_rect.width() > 0.0
            && let Some(pos) = hist_resp.interact_pointer_pos()
        {
            let norm = ((pos.x - hist_rect.left()) / hist_rect.width()).clamp(0.0, 1.0) as f64;
            let centre = state.bounds_z_min + norm * span;
            let half = ((state.clip_z_max - state.clip_z_min) * 0.5).max(0.02);
            let low = (centre - half).clamp(state.bounds_z_min, state.bounds_z_max);
            state.clip_z_min = low;
            state.clip_z_max = (low + half * 2.0).min(state.bounds_z_max);
            actions.push(PointCloudUiAction::SetZClip { z_min: state.clip_z_min, z_max: state.clip_z_max });
        }
        ui.add_space(4.0);

        // The two sliders. Their ranges are ordered, so egui never gets a range that starts above
        // its end.
        let thickness = (state.clip_z_max - state.clip_z_min).max(0.02);
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("Min Z:");
            let top = if state.lock_slice_thickness {
                (state.bounds_z_max - thickness).max(state.bounds_z_min)
            } else {
                state.clip_z_max.max(state.bounds_z_min)
            };
            if ui.add(egui::Slider::new(&mut state.clip_z_min, state.bounds_z_min..=top).suffix(" m").fixed_decimals(2)).changed() {
                if state.lock_slice_thickness {
                    state.clip_z_max = (state.clip_z_min + thickness).min(state.bounds_z_max);
                }
                changed = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label("Max Z:");
            let bottom = if state.lock_slice_thickness {
                (state.bounds_z_min + thickness).min(state.bounds_z_max)
            } else {
                state.clip_z_min.min(state.bounds_z_max)
            };
            if ui.add(egui::Slider::new(&mut state.clip_z_max, bottom..=state.bounds_z_max).suffix(" m").fixed_decimals(2)).changed() {
                if state.lock_slice_thickness {
                    state.clip_z_min = (state.clip_z_max - thickness).max(state.bounds_z_min);
                }
                changed = true;
            }
        });

        // The jogger: the band walks up and down with its thickness kept.
        ui.horizontal(|ui| {
            ui.checkbox(&mut state.lock_slice_thickness, "Lock slice").on_hover_text("Keep the band's thickness while it moves");
            ui.label(RichText::new(format!("({:.2} m)", thickness)).small().weak());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(format!("+{:.2} m", state.jog_step_m)).on_hover_text("Raise the band").clicked() {
                    let step = state.jog_step_m.min((state.bounds_z_max - state.clip_z_max).max(0.0));
                    state.clip_z_min += step;
                    state.clip_z_max += step;
                    changed = true;
                }
                if ui.button(format!("-{:.2} m", state.jog_step_m)).on_hover_text("Lower the band").clicked() {
                    let step = state.jog_step_m.min((state.clip_z_min - state.bounds_z_min).max(0.0));
                    state.clip_z_min -= step;
                    state.clip_z_max -= step;
                    changed = true;
                }
            });
        });
        if changed {
            actions.push(PointCloudUiAction::SetZClip { z_min: state.clip_z_min, z_max: state.clip_z_max });
        }
    });
}

/// An eye, drawn in code (the repo has no image assets).
fn draw_eye_icon(ui: &mut Ui, opacity: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(18.0, 16.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter_at(rect);
    let centre = rect.center();
    let alpha = (opacity.clamp(0.25, 1.0) * 255.0) as u8;
    let colour = Color32::from_rgba_unmultiplied(200, 210, 225, alpha);
    painter.circle_stroke(centre, 5.5, Stroke::new(1.3, colour));
    painter.circle_filled(centre, 2.3, colour);
}

/// The pivot gizmo, drawn over the 3D view while the camera turns around a point.
pub fn draw_orbit_pivot(painter: &Painter, pivot: Pos2) {
    let r = 9.0;
    let gold = Color32::from_rgb(255, 195, 55);
    painter.circle_stroke(pivot, r, Stroke::new(1.5, gold));
    painter.circle_filled(pivot, 2.5, gold);
    painter
        .line_segment([Pos2::new(pivot.x - r - 4.0, pivot.y), Pos2::new(pivot.x - 4.0, pivot.y)], Stroke::new(1.5, Color32::from_rgb(235, 85, 85)));
    painter
        .line_segment([Pos2::new(pivot.x, pivot.y - r - 4.0), Pos2::new(pivot.x, pivot.y - 4.0)], Stroke::new(1.5, Color32::from_rgb(85, 200, 255)));
    painter.text(Pos2::new(pivot.x + 12.0, pivot.y - 10.0), Align2::LEFT_BOTTOM, "PIVOT", FontId::monospace(10.0), gold);
}
