//! The point cloud's viewport HUD: a floating bar over the 3D view, and the Z band panel with
//! the scan's height histogram.
//!
//! The panel and the bar are thin: they read the session's state and push [`PointCloudUiAction`]s,
//! which the caller turns into commands (`LASCLIP`, `LASOPACITY`, `LASFRAME`) or into view state
//! (`app.view3d`). Nothing here owns state of its own, so the palette, the bar and the command
//! line can never disagree.

use cadcraft_pointcloud::{ALL_CLASSES, Cloud, CloudColor, DEVIATION_NO_DATA, Spectrum, class_color, class_name, deviation_color};
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
    egui::Area::new(Id::unique("rencad_pointcloud_hud")).order(Order::Foreground).anchor(Align2::CENTER_TOP, Vec2::new(0.0, 66.0)).show(ctx, |ui| {
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
        let min_norm = ((state.clip_z_min - state.bounds_z_min) / span).clamp(0.0, 1.0) as f32;
        let max_norm = ((state.clip_z_max - state.bounds_z_min) / span).clamp(0.0, 1.0) as f32;
        if ui.is_rect_visible(hist_rect) {
            draw_histogram(&ui.painter_at(hist_rect), hist_rect, &state.z_histogram, Some((min_norm, max_norm)));
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

/// The bars of a scan histogram, with the window the ramp reads marked. `window` is a fraction of
/// the width, so the same drawer shows the Z cut and the colour window over either attribute.
fn draw_histogram(painter: &Painter, rect: Rect, counts: &[u32], window: Option<(f32, f32)>) {
    painter.rect_filled(rect, 4.0, Color32::from_rgb(20, 22, 27));
    let max_count = counts.iter().copied().max().unwrap_or(1).max(1) as f32;
    let bins = counts.len() as f32;
    let bin_w = rect.width() / bins;
    for (i, &count) in counts.iter().enumerate() {
        let centre = (i as f32 + 0.5) / bins;
        let inside = window.is_none_or(|(lo, hi)| centre >= lo && centre <= hi);
        // The square root keeps the thin bins (a wall) visible next to the floor and the ceiling.
        let ratio = (count as f32 / max_count).sqrt().clamp(0.0, 1.0);
        let bar_h = ratio * (rect.height() - 6.0);
        let x0 = rect.left() + i as f32 * bin_w;
        let bar = Rect::from_min_max(Pos2::new(x0 + 0.5, rect.bottom() - bar_h), Pos2::new(x0 + bin_w - 0.5, rect.bottom()));
        let colour = if inside { Color32::from_rgb(76, 162, 245) } else { Color32::from_rgb(52, 57, 68) };
        painter.rect_filled(bar, 1.0, colour);
    }
    if let Some((lo, hi)) = window {
        let edges =
            [(rect.left() + lo * rect.width(), Color32::from_rgb(90, 210, 140)), (rect.left() + hi * rect.width(), Color32::from_rgb(245, 155, 85))];
        for (x, colour) in edges {
            painter.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], Stroke::new(2.0, colour));
        }
    }
}

/// The band's returns by intensity, with the colour window marked: the same 64 bins the ramp
/// reads, so the width of the window is visible against where the bright walls actually sit.
pub fn draw_intensity_histogram(ui: &mut Ui, cloud: &Cloud) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 46.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let bins = cloud.ihisto.len() as f32;
    let (lo, hi) = cloud.intensity_window();
    let window = (cloud.bin_of_intensity(lo) as f32 / bins, (cloud.bin_of_intensity(hi) + 1) as f32 / bins);
    draw_histogram(&ui.painter_at(rect), rect, &cloud.ihisto, Some(window));
}

/// One swatch of the colour legend: the colour the points are really drawn in, and what it means.
#[derive(Debug, Clone, PartialEq)]
pub struct LegendChip {
    pub color: [u8; 3],
    pub label: String,
}

/// The swatches a ramp legend shows. Seven is the most that keeps the ends readable in a column
/// the width of a palette.
const RAMP_CHIPS: usize = 7;

/// The classes a cloud actually carries, in the order the classifier lists them. Every level is
/// walked, not sampled: a class with three points still needs its chip.
pub fn classes_present(cloud: &Cloud) -> Vec<u8> {
    let mut seen = [false; 256];
    for level in &cloud.levels {
        for code in &level.classes {
            seen[usize::from(*code)] = true;
        }
    }
    // The table's own order first, so a classified scan reads the way a report lists its classes.
    let mut codes: Vec<u8> = ALL_CLASSES.iter().copied().filter(|code| seen[usize::from(*code)]).collect();
    // Then any code the file carries that our classifier never writes. ASPRS 0 — created and never
    // classified — is the usual state of a cleaned export, and its grey needs a name too.
    codes.extend((0u8..=255).filter(|code| seen[usize::from(*code)] && !ALL_CLASSES.contains(code)));
    codes
}

/// The ramp's own colours, at evenly spaced points along it, with only its ends named.
fn ramp_legend(spectrum: Spectrum, low: &str, high: &str) -> Vec<LegendChip> {
    let mut chips = Vec::with_capacity(RAMP_CHIPS);
    for i in 0..RAMP_CHIPS {
        let label = if i == 0 {
            low
        } else if i + 1 == RAMP_CHIPS {
            high
        } else {
            ""
        };
        chips.push(LegendChip { color: spectrum.apply(i as f32 / (RAMP_CHIPS - 1) as f32), label: label.to_string() });
    }
    chips
}

/// The comparison ramp, sampled through the function the points themselves are coloured by: the
/// spectrum's diverging stops are a near-copy of it and can be a unit off, and a legend that is a
/// unit off is a legend that lies.
fn deviation_legend(scale: f32) -> Vec<LegendChip> {
    let high = format!("{scale:.2} m");
    let mut chips = Vec::with_capacity(RAMP_CHIPS + 1);
    for i in 0..RAMP_CHIPS {
        let t = i as f32 / (RAMP_CHIPS - 1) as f32;
        let label = if i == 0 {
            "0.00 m".to_string()
        } else if i + 1 == RAMP_CHIPS {
            high.clone()
        } else {
            String::new()
        };
        chips.push(LegendChip { color: deviation_color(t * scale, scale), label });
    }
    // The point the reference has nothing for is drawn in this grey — a comparison at the edge of a
    // wall is mostly it — so the key has to carry it, not only the ramp.
    chips.push(LegendChip { color: DEVIATION_NO_DATA, label: "no data".into() });
    chips
}

/// What the cloud's colours mean, in the mode it is drawn in — built with the same maths the
/// points use, so a legend cannot promise a colour the drawing does not produce. A photo and a
/// single flat colour have nothing to explain.
pub fn color_legend(cloud: &Cloud) -> Vec<LegendChip> {
    match cloud.color {
        CloudColor::Elevation => {
            let (lo, hi) = cloud.elevation_window();
            ramp_legend(cloud.spectrum, &format!("{lo:.2} m"), &format!("{hi:.2} m"))
        }
        CloudColor::Intensity => {
            let (lo, hi) = cloud.intensity_window();
            ramp_legend(cloud.spectrum, &lo.to_string(), &hi.to_string())
        }
        // The comparison ramp is fixed: zero deviation to the scale the user set.
        CloudColor::Deviation => deviation_legend(cloud.dev_scale),
        CloudColor::Class => classes_present(cloud)
            .into_iter()
            .map(|code| LegendChip {
                color: class_color(code),
                // A code outside the table has no name of its own, so the legend gives its number:
                // a surveyor reads ASPRS 0 as "never classified" from the code, not from us.
                label: if ALL_CLASSES.contains(&code) { class_name(code).to_string() } else { format!("class {code}") },
            })
            .collect(),
        CloudColor::Rgb | CloudColor::Single(_) => Vec::new(),
    }
}

/// The legend as one wrapped row of swatches: a key that sits under the sliders it explains, with
/// no dialog to open and no asset to ship.
pub fn draw_color_legend(ui: &mut Ui, cloud: &Cloud) {
    let chips = color_legend(cloud);
    if chips.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for chip in chips {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(14.0, 11.0), Sense::hover());
            if ui.is_rect_visible(rect) {
                let painter = ui.painter_at(rect);
                let colour = Color32::from_rgb(chip.color[0], chip.color[1], chip.color[2]);
                painter.rect_filled(rect, 2.0, colour);
                painter.rect_stroke(rect, 2.0, Stroke::new(0.5, Color32::from_rgb(70, 76, 88)), egui::StrokeKind::Inside);
            }
            if chip.label.is_empty() {
                ui.add_space(1.0);
            } else {
                ui.label(RichText::new(chip.label).size(10.0));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadcraft_pointcloud::{Bounds3, Cloud, CloudColor, CloudVtx, Pt, Slice, Spectrum, class_color, class_name, deviation_color};

    /// A wall of 60 points: six across, ten up, every third point a different class, so the
    /// legends have a real band's statistics to read.
    fn scan(color: CloudColor) -> Cloud {
        scan_with(color, |i| match i % 3 {
            0 => 2,
            1 => 6,
            _ => 1,
        })
    }

    /// The same wall with the classes a caller picks — a file that was never classified carries one
    /// code on every point.
    fn scan_with(color: CloudColor, class: impl Fn(i32) -> u8) -> Cloud {
        let pts: Vec<_> = (0..60)
            .map(|i| Pt {
                x: f64::from(i % 6) * 0.05,
                y: f64::from(i / 6) * 0.05,
                z: f64::from(i) * 0.02,
                rgb: None,
                intensity: u16::try_from(i * 1100).unwrap_or(0),
                class: class(i),
            })
            .collect();
        let mut b = Bounds3::EMPTY;
        for p in &pts {
            b.add(p.x, p.y, p.z);
        }
        let slice = Slice { points: pts, total: 60, in_band: 60, bounds: b };
        Cloud::from_slice("wall".into(), String::new(), 0.0, 1.6, Some(0.05), slice, 1_000_000, color)
    }

    #[test]
    fn the_elevation_legend_marks_the_window_the_ramp_reads() {
        let mut c = scan(CloudColor::Elevation);
        c.spectrum = Spectrum::Grey;
        c.z_window = Some((1.0, 2.0));
        let legend = color_legend(&c);
        assert_eq!(legend.first().unwrap().label, "1.00 m", "the ramp starts where the user said");
        assert_eq!(legend.last().unwrap().label, "2.00 m", "and ends where the user said");
        let last = (legend.len() - 1) as f32;
        for (i, chip) in legend.iter().enumerate() {
            assert_eq!(chip.color, Spectrum::Grey.apply(i as f32 / last), "chip {i} is the colour the pixels get");
        }
    }

    /// The auto window is the band's 2%–98% slice, not its full height: a legend that named the
    /// full height would promise colours the points never get.
    #[test]
    fn the_auto_window_labels_the_elevation_legend() {
        let c = scan(CloudColor::Elevation);
        let (lo, hi) = c.elevation_window();
        let legend = color_legend(&c);
        assert_eq!(legend.first().unwrap().label, format!("{lo:.2} m"));
        assert_eq!(legend.last().unwrap().label, format!("{hi:.2} m"));
    }

    #[test]
    fn the_intensity_legend_labels_the_display_window() {
        let mut c = scan(CloudColor::Intensity);
        c.i_window = Some((51, 203));
        let legend = color_legend(&c);
        assert_eq!(legend.first().unwrap().label, "51", "the intensity ramp reads the 0..255 scale");
        assert_eq!(legend.last().unwrap().label, "203");
        assert!(legend[1..legend.len() - 1].iter().all(|chip| chip.label.is_empty()), "only the ends are named");
    }

    /// The class legend lists what the scan actually carries, in the order the classifier documents
    /// them — a wall of ground and buildings shows two chips, not twelve.
    #[test]
    fn the_class_legend_lists_only_the_classes_the_scan_carries() {
        let c = scan(CloudColor::Class);
        assert_eq!(classes_present(&c), vec![1, 2, 6]);
        let legend = color_legend(&c);
        assert_eq!(legend.len(), 3);
        for (chip, code) in legend.iter().zip([1u8, 2, 6]) {
            assert_eq!(chip.label, class_name(code));
            assert_eq!(chip.color, class_color(code), "the chip is the colour the class is drawn in");
        }
    }

    /// A cleaned export carries ASPRS code 0 — "created, never classified" — on every point. The
    /// classifier never writes that code, so a legend built only from its table shows nothing at all
    /// and the grey cloud explains nothing.
    #[test]
    fn the_class_legend_names_a_class_the_classifier_never_writes() {
        let c = scan_with(CloudColor::Class, |_| 0);
        assert_eq!(classes_present(&c), vec![0], "the code the file carries is a code the cloud has");
        let legend = color_legend(&c);
        assert_eq!(legend.len(), 1, "one class, one chip");
        assert_eq!(legend[0].color, class_color(0), "and the chip is the grey the points are drawn in");
        assert!(legend[0].label.contains('0'), "the label names the code, not just `other`: {}", legend[0].label);
    }

    #[test]
    fn the_deviation_legend_ends_at_the_scale_the_points_are_read_against() {
        let mut c = scan(CloudColor::Deviation);
        c.dev_scale = 0.05;
        let legend = color_legend(&c);
        // The ramp runs to the no-data chip, which is the grey, not a step of the scale.
        let ramp = &legend[..legend.len() - 1];
        assert_eq!(ramp.first().unwrap().label, "0.00 m");
        assert_eq!(ramp.last().unwrap().label, "0.05 m");
        let last = (ramp.len() - 1) as f32;
        for (i, chip) in ramp.iter().enumerate() {
            assert_eq!(chip.color, deviation_color(i as f32 / last * 0.05, 0.05), "chip {i} matches the shader's own maths");
        }
    }

    /// A point that finds no match is drawn in the neutral grey, whatever the ramp: the key has to
    /// name that grey too, or the drawing produces a colour the legend does not explain.
    #[test]
    fn the_deviation_legend_names_the_colour_of_a_point_that_found_nothing() {
        let c = scan(CloudColor::Deviation);
        let grey = c.color_of(&CloudVtx { x: 0.0, y: 0.0, z: 0.0, rgb: [0, 0, 0], intensity: 0 }, None, None);
        let legend = color_legend(&c);
        assert!(legend.iter().any(|chip| chip.color == grey && !chip.label.is_empty()), "the grey a point with no match gets is named: {legend:?}");
    }

    /// A photo needs no legend: the colours are the scan's own, and one flat colour explains
    /// itself.
    #[test]
    fn rgb_and_single_colour_need_no_ramp_legend() {
        for color in [CloudColor::Rgb, CloudColor::Single([200, 30, 40])] {
            assert!(color_legend(&scan(color)).is_empty(), "{color:?} has no ramp to explain");
        }
    }

    /// The profile marks the window it is read through: the bins inside are lit and the rest are
    /// dim, so the width of the window is visible against where the returns actually sit.
    #[test]
    fn the_intensity_profile_lights_only_the_window_it_draws() {
        let mut c = scan(CloudColor::Intensity);
        c.i_window = Some((0, 31));
        let ctx = egui::Context::default();
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 120.0))), ..Default::default() };
        let mut bars = Vec::new();
        for _ in 0..2 {
            let mut out = ctx.run_ui(input.clone(), |ui| draw_intensity_histogram(ui, &c));
            bars = out
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Rect(r) if r.rect.height() > 0.5 && r.rect.width() < 20.0 => Some(r.fill),
                    _ => None,
                })
                .collect();
            out.textures_delta.clear();
        }
        let lit = Color32::from_rgb(76, 162, 245);
        let dim = Color32::from_rgb(52, 57, 68);
        let (lit_bars, dim_bars) = (bars.iter().filter(|c| **c == lit).count(), bars.iter().filter(|c| **c == dim).count());
        assert!(lit_bars > 0 && lit_bars < 16, "the bottom eighth of the scale is lit: {lit_bars} bars");
        assert!(dim_bars > 0, "the rest of the profile stays dim");
    }
}
