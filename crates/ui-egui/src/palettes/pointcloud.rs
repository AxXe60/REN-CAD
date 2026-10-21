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
    let (points, levels, in_band, color, cell_now, bounds, clip, imin, imax) = (
        cloud.points(),
        cloud.levels.len(),
        cloud.full_points,
        cloud.color,
        cloud.levels.first().map_or(0.0, |l| l.cell),
        cloud.bounds,
        cloud.clip,
        cloud.imin,
        cloud.imax,
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

    // ---------- info ----------
    ui.add_space(8.0);
    let shown = if all { format!("{} points (all)", num(points)) } else { format!("{} of {} band points", num(points), num(in_band)) };
    let intensity = if color == cadcraft_pointcloud::CloudColor::Intensity { format!(" · intensity {imin}–{imax}") } else { String::new() };
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(format!("{shown} · {levels} levels{intensity}")).color(t.text_dim).size(11.0));
    });
    if app.view3d.active {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(format!("3D: {} points on screen", num(app.view3d.drawn))).color(t.text_dim).size(11.0));
        });
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            if ui.button("Fit 3D").clicked() {
                app.view3d.fitted = false;
            }
            let mut size = app.view3d.point_size;
            if ui.add(egui::Slider::new(&mut size, 0.5..=4.0).fixed_decimals(1).text("pt")).changed() {
                app.view3d.point_size = size;
            }
        });
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("to draw over it, switch back to 2D").color(t.text_dim).size(11.0));
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
