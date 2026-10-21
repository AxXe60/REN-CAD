//! The Point Cloud palette: clouds opened with LASOPEN, their colour, decimation and the 3D view.
//!
//! It is a thin panel: every action goes through the same engine commands the command line uses
//! (`LASOPEN`, `LASCLOSE`, `LASFRAME`, `LASCOLOR`, `LASDECIMATE`, `LASVISIBLE`), so the palette
//! and a script cannot drift apart. Only the 3D toggle is UI state, because the camera is a view,
//! not a drawing property.

use serde_json::json;

use crate::CadApp;
use crate::theme::Tokens;

/// Thousands separators for the point counts: 1 394 540.
fn num(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

pub fn section(app: &mut CadApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    super::section_header(ui, "Point Cloud");
    let sel_id = egui::Id::new("cc_pc_selected");
    let path_id = egui::Id::new("cc_pc_path");
    let cell_id = egui::Id::new("cc_pc_cell");
    let pick_id = egui::Id::new("cc_pc_color");

    if app.session.clouds.is_empty() {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("No cloud open.").color(t.text_dim).size(12.0));
        });
        let mut path = ui.data(|d| d.get_temp::<String>(path_id)).unwrap_or_default();
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            let edit = egui::TextEdit::singleline(&mut path).hint_text("path to .las / .laz").desired_width(ui.available_width() - 70.0);
            ui.add(edit);
            if ui.button("Open").on_hover_text("Open at full resolution; the Grid slider thins it").clicked() {
                open(app, &path);
            }
        });
        ui.data_mut(|d| d.insert_temp(path_id, path));
        return;
    }

    let selected = app.session.clouds.len().saturating_sub(1);
    let selected = ui.data(|d| d.get_temp::<usize>(sel_id)).filter(|i| *i < app.session.clouds.len()).unwrap_or(selected);
    let cloud = &app.session.clouds[selected];
    let (points, levels, in_band, color, cell_now, bounds, clip, imin, imax, crs) = (
        cloud.points(),
        cloud.levels.len(),
        cloud.full_points,
        cloud.color,
        cloud.levels.first().map_or(0.0, |l| l.cell),
        cloud.bounds,
        cloud.clip,
        cloud.imin,
        cloud.imax,
        cloud.crs.clone(),
    );
    let visible = app.session.clouds[selected].visible;

    // ---------- actions ----------
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        if ui.button("Fit").on_hover_text("Frame the cloud in the drawing").clicked() {
            run(app, "lasframe", json!({"index": selected}));
        }
        let label = if app.view3d.active { "2D" } else { "3D" };
        if ui.button(label).on_hover_text("Point cloud 3D view (orbit, pan, zoom)").clicked() {
            app.view3d.active = !app.view3d.active;
            app.view3d.fitted = false;
        }
        if ui.button("Close").clicked() {
            run(app, "lasclose", json!({"index": selected}));
        }
        if ui.checkbox(&mut { visible }, "Show").clicked() {
            run(app, "lasvisible", json!({"index": selected, "visible": !visible}));
        }
    });

    // ---------- the list ----------
    ui.add_space(6.0);
    let mut pick: Option<usize> = None;
    ui.scope(|ui| {
        for (i, c) in app.session.clouds.iter().enumerate() {
            let active = i == selected;
            let text = format!("{}  ·  {} pts  ·  {} LOD", c.name, num(c.points()), c.levels.len());
            let row = ui.selectable_label(active, egui::RichText::new(text).size(12.0));
            if row.clicked() {
                pick = Some(i);
            }
            if !c.visible {
                row.on_hover_text("Hidden");
            }
        }
    });
    if let Some(i) = pick {
        ui.data_mut(|d| d.insert_temp(sel_id, i));
    }

    // ---------- colour ----------
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Colour").color(t.text_dim).size(12.0));
        for (mode, label) in [("rgb", "RGB"), ("elevation", "Elevation"), ("intensity", "Intensity")] {
            if ui.selectable_label(color.name() == mode, label).clicked() {
                run(app, "lascolor", json!({"index": selected, "mode": mode}));
            }
        }
        let mut single = ui.data(|d| d.get_temp::<[u8; 3]>(pick_id)).unwrap_or([230, 230, 230]);
        if ui.color_edit_button_srgb(&mut single).changed() {
            ui.data_mut(|d| d.insert_temp(pick_id, single));
            let mode = format!("single:#{:02x}{:02x}{:02x}", single[0], single[1], single[2]);
            run(app, "lascolor", json!({"index": selected, "mode": mode}));
        }
    });

    // ---------- resolution ----------
    ui.add_space(6.0);
    let all = cell_now <= 0.0;
    let mut cell = ui.data(|d| d.get_temp::<f32>(cell_id)).unwrap_or(if all { 0.005 } else { cell_now as f32 });
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Grid").color(t.text_dim).size(12.0));
        let slider = egui::Slider::new(&mut cell, 0.001..=0.2).logarithmic(true).suffix(" m").fixed_decimals(3);
        if ui.add(slider).changed() {
            ui.data_mut(|d| d.insert_temp(cell_id, cell));
        }
        if ui.button("Apply").on_hover_text("Re-read the file at this grid cell").clicked() {
            run(app, "lasdecimate", json!({"index": selected, "cell": cell}));
        }
        if ui.selectable_label(all, "All").on_hover_text("Every point in the LAS band, no grid").clicked() {
            run(app, "lasdecimate", json!({"index": selected, "cell": 0.0}));
        }
    });

    // ---------- crop: walls and ceiling ----------
    ui.add_space(6.0);
    let mut enabled = clip.is_some();
    let mut min = clip.map_or(bounds.min, |c| c.min);
    let mut max = clip.map_or(bounds.max, |c| c.max);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let box_on = ui
            .checkbox(&mut enabled, "Remove walls / ceiling")
            .on_hover_text(
                "Crop box: raise the lower Z to drop the floor, lower the upper Z to drop the ceiling, pull X and Y in to remove the walls",
            )
            .changed();
        if box_on {
            if enabled {
                run(app, "lasclip", json!({"index": selected, "min": min, "max": max}));
            } else {
                run(app, "lasclip", json!({"index": selected, "reset": true}));
            }
        }
    });
    if enabled {
        let mut changed = false;
        for (axis, k) in [("X", 0usize), ("Y", 1), ("Z", 2)] {
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                ui.label(egui::RichText::new(axis).color(t.text_dim).size(11.0));
                changed |= ui.add(egui::DragValue::new(&mut min[k]).speed(0.01).range(bounds.min[k]..=bounds.max[k]).max_decimals(3)).changed();
                ui.label("..");
                changed |= ui.add(egui::DragValue::new(&mut max[k]).speed(0.01).range(bounds.min[k]..=bounds.max[k]).max_decimals(3)).changed();
            });
        }
        if changed {
            for k in 0..3 {
                min[k] = min[k].min(max[k]);
            }
            run(app, "lasclip", json!({"index": selected, "min": min, "max": max}));
        }
    }

    // ---------- the ground under the cloud ----------
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        if ui
            .button("Find ground")
            .on_hover_text("Find the floor under the scan: the lowest point of every cell seeds a surface, and the points on it are the ground")
            .clicked()
        {
            run(app, "lasground", json!({ "index": selected }));
        }
        let mut only = app.session.clouds[selected].ground_only;
        if ui.checkbox(&mut only, "Ground only").on_hover_text("Draw only the points the ground filter found").changed() {
            run(app, "lasground", json!({ "index": selected, "show": only }));
        }
    });
    // How opaque the cloud is: below 1 the 3D view thins the points (the x-ray), so the drawing
    // under the cloud stays readable.
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Opacity").color(t.text_dim).size(12.0));
        let mut opacity = app.session.clouds[selected].opacity;
        if ui
            .add_sized([150.0, 18.0], egui::Slider::new(&mut opacity, 0.05..=1.0).fixed_decimals(2).suffix(" ×"))
            .on_hover_text("How opaque the cloud is drawn. Below 1 the points are thinned, so a wall seen edge-on does not hide the drawing under it")
            .changed()
        {
            run(app, "lasopacity", json!({ "index": selected, "opacity": opacity }));
        }
    });

    // ---------- info ----------
    ui.add_space(8.0);
    let shown = if all { format!("{} points (all)", num(points)) } else { format!("{} of {} band points", num(points), num(in_band)) };
    let intensity = if color == cadcraft_pointcloud::CloudColor::Intensity { format!(" · intensity {imin}–{imax}") } else { String::new() };
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(format!("{shown} · {levels} levels{intensity}")).color(t.text_dim).size(11.0));
    });
    // Where the scan says it is: the EPSG code and the system's name.
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let crs = app.session.crs.clone().or_else(|| crs.clone());
        let text = match &crs {
            Some(crs) => {
                let mut text = match (crs.epsg, crs.name()) {
                    (Some(code), Some(name)) => format!("CRS: EPSG:{code} · {name}"),
                    (Some(code), None) => format!("CRS: EPSG:{code}"),
                    (None, _) => "CRS: the file names a system, but no EPSG code".to_string(),
                };
                // The local grid's offsets: a survey office that writes the northing without its
                // leading millions shows it here.
                if crs.false_easting != 0.0 || crs.false_northing != 0.0 {
                    text.push_str(&format!(" · E {:+.0} N {:+.0}", crs.false_easting, crs.false_northing));
                }
                text
            }
            None => "CRS: the file names none".to_string(),
        };
        ui.label(egui::RichText::new(text).color(t.text_dim).size(11.0));
    });
    // The site frame: the drawing's (0, 0, 0) is that point in the world.
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let o = app.session.local_origin;
        let text = if o == [0.0; 3] {
            "Site origin: the drawing's (0, 0, 0) is the world's (0, 0, 0)".to_string()
        } else {
            format!("Site origin: {:.3}, {:.3}, {:.3}", o[0], o[1], o[2])
        };
        ui.label(egui::RichText::new(text).color(t.text_dim).size(11.0));
    });
    if app.view3d.active {
        // ---------- 3D drafting (first: it is what the view is for) ----------
        let strokes = app.session.sketch.count();
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            let drawing = app.view3d.draft.active && app.view3d.draft.tool == crate::view3d::DraftTool::Draw;
            let hint = format!("Click in the 3D view to place vertices; each click snaps to the cloud ({strokes} strokes so far)");
            if ui.selectable_label(drawing, "Draw 3D").on_hover_text(hint).clicked() {
                app.view3d.draft.active = !drawing;
                app.view3d.draft.tool = crate::view3d::DraftTool::Draw;
                app.view3d.draft.cancel();
            }
            let measuring = app.view3d.draft.active && app.view3d.draft.tool == crate::view3d::DraftTool::Measure;
            if ui
                .selectable_label(measuring, "Measure")
                .on_hover_text("Click two points to measure: the length, the run in plan and the rise")
                .clicked()
            {
                app.view3d.draft.active = !measuring;
                app.view3d.draft.tool = crate::view3d::DraftTool::Measure;
                app.view3d.draft.cancel();
            }
            if ui.small_button("Undo").on_hover_text("Drop the last 3D stroke (3DUNDO)").clicked() {
                run(app, "3dundo", json!({}));
            }
            if ui.small_button("Clear").on_hover_text("Drop every 3D stroke (3DCLEAR); 3DEXPORT saves them as DXF lines with Z").clicked() {
                run(app, "3dclear", json!({}));
            }
        });

        if app.view3d.draft.active {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                let placed = app.view3d.draft.points.len();
                let measuring = app.view3d.draft.tool == crate::view3d::DraftTool::Measure;
                if ui
                    .add_enabled(placed > 0 && !measuring, egui::Button::new("Finish"))
                    .on_hover_text("End the stroke (double-click or right-click in the view)")
                    .clicked()
                {
                    crate::view3d::finish_draft(app, false);
                }
                if ui
                    .add_enabled(placed > 1 && !measuring, egui::Button::new("Close"))
                    .on_hover_text("Close the stroke: the last vertex joins the first (C in the view)")
                    .clicked()
                {
                    crate::view3d::finish_draft(app, true);
                }
                if ui.button("Cancel").on_hover_text("Drop the stroke being drawn (Esc in the view)").clicked() {
                    crate::view3d::cancel_draft(app);
                }
                let mut z_lock = app.view3d.draft.z_lock;
                if ui
                    .checkbox(&mut z_lock, "Z lock")
                    .on_hover_text("Hold the stroke in the horizontal plane of its first vertex (Z in the view)")
                    .changed()
                {
                    app.view3d.draft.z_lock = z_lock;
                }
                let mut ortho = app.view3d.draft.ortho;
                if ui
                    .checkbox(&mut ortho, "Ortho")
                    .on_hover_text("Every segment follows the cardinal axis it is closest to (Shift for one move)")
                    .changed()
                {
                    app.view3d.draft.ortho = ortho;
                }
            });
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                for (mode, label, hint) in [
                    (
                        cadcraft_pointcloud::SnapMode::Smooth,
                        "Smooth",
                        "Snap to the average of the points around the front one: the scan noise cancels out",
                    ),
                    (cadcraft_pointcloud::SnapMode::Exact, "Exact", "Snap to the cloud point itself, exactly under the cursor"),
                ] {
                    if ui.selectable_label(app.session.snap3d == mode, label).on_hover_text(hint).clicked() {
                        run(app, "3dsnap", json!({ "mode": mode.name() }));
                    }
                }
                // The layer the finished stroke is written on.
                let mut layer = app.view3d.draft.layer.clone();
                let edit = egui::TextEdit::singleline(&mut layer).desired_width(70.0).hint_text("layer");
                if ui.add(edit).on_hover_text("The DXF layer the finished stroke is written on").changed() {
                    app.view3d.draft.layer = layer;
                }
            });
        }

        // ---------- camera ----------
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            for (mode, label, hint) in [
                (crate::view3d::Projection::Perspective, "Persp", "Perspective with a long lens: a little depth, no fish-eye"),
                (crate::view3d::Projection::Ortho, "Ortho", "Parallel projection: walls stay parallel and distances are true at any depth"),
            ] {
                if ui.selectable_label(app.view3d.projection == mode, label).on_hover_text(hint).clicked() {
                    app.view3d.projection = mode;
                }
            }
            if ui.button("Fit").on_hover_text("Frame the clouds (double-click in the view does the same)").clicked() {
                app.view3d.fitted = false;
            }
            for (label, hint, yaw, pitch) in [
                ("Top", "Look down: X right, Y up", -std::f64::consts::FRAC_PI_2, 1.5),
                ("Front", "Look along +Y: X right, Z up", -std::f64::consts::FRAC_PI_2, 0.0),
                ("Iso", "The three-quarter default", -2.2, 0.55),
            ] {
                if ui.small_button(label).on_hover_text(hint).clicked() {
                    app.view3d.preset(yaw, pitch);
                }
            }
        });
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Lens").color(t.text_dim).size(12.0));
            let mut lens = app.view3d.fov_y.to_degrees();
            let slider = egui::Slider::new(&mut lens, 15.0..=90.0).suffix("°").fixed_decimals(0);
            if ui
                .add_sized([150.0, 18.0], slider)
                .on_hover_text("Vertical field of view: 35° is a normal lens, 15° a telephoto, 90° wide angle")
                .changed()
            {
                app.view3d.fov_y = lens.to_radians();
            }
        });
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Size").color(t.text_dim).size(12.0));
            let mut size = app.view3d.point_size;
            if ui.add_sized([150.0, 18.0], egui::Slider::new(&mut size, 0.5..=4.0).fixed_decimals(1).suffix(" pt")).changed() {
                app.view3d.point_size = size;
            }
        });

        // ---------- clarity without a depth buffer ----------
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Cut").color(t.text_dim).size(12.0));
            let mut cut = app.view3d.front_cut;
            if ui
                .add_sized([150.0, 18.0], egui::Slider::new(&mut cut, 0.0..=0.9).fixed_decimals(2).suffix(" d"))
                .on_hover_text("Front wall cut: drop the points between the camera and this fraction of the distance to the target, so you can look into a room without cropping the walls out of the cloud")
                .changed()
            {
                app.view3d.front_cut = cut;
            }
        });
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Cue").color(t.text_dim).size(12.0));
            let mut cue = app.view3d.depth_cue;
            if ui
                .add_sized([150.0, 18.0], egui::Slider::new(&mut cue, 0.0..=1.0).fixed_decimals(2).suffix(" ×"))
                .on_hover_text("Depth cue: the further a point is behind the target, the more it fades into the background and the smaller it gets")
                .changed()
            {
                app.view3d.depth_cue = cue;
            }
        });
    }
}

fn open(app: &mut CadApp, path: &str) {
    let path = path.trim();
    if path.is_empty() {
        return;
    }
    // No `decimate`: the finest level is the file's own points (LASOPEN thin it only if the band
    // is over the 8-million read cap).
    run(app, "lasopen", json!({"path": path}));
}

fn run(app: &mut CadApp, command: &str, params: serde_json::Value) {
    if let Err(error) = app.run(command, params) {
        app.session.echo(error.clone());
        app.set_status(error);
    }
}
