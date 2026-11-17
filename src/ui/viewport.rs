use crate::app::TekkCamApp;
use crate::renderer::pipeline::Scene;
use crate::renderer::ViewportCallback;
use eframe::egui;
use std::sync::Arc;

pub fn show(ctx: &egui::Context, app: &mut TekkCamApp) {
    let viewport_bg = if app.ui_state.theme == crate::app::AppTheme::Light {
        egui::Color32::from_rgb(250, 250, 250)
    } else {
        egui::Color32::from_rgb(10, 10, 15)
    };
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(viewport_bg))
        .show(ctx, |ui| {
            let rect = ui.available_rect_before_wrap();
            app.ui_state.viewport_size = rect.size();

            if !app.has_active_project() {
                show_project_welcome(ui, app, rect);
                return;
            }

            // Apply orbit input first so the scene builder can use its bounded
            // preview mesh while a right-button drag is active.
            let orbit_preview = handle_camera_input(ui, app, rect);
            sync_scene(app, orbit_preview);
            if app.ui_state.face_selection_mode {
                ui.painter_at(rect).text(
                    egui::pos2(rect.center().x, rect.top() + 12.0),
                    egui::Align2::CENTER_TOP,
                    "Face selection active · click faces to toggle selection",
                    egui::FontId::proportional(14.0),
                    egui::Color32::from_rgb(90, 225, 235),
                );
            }

            // Frame all geometry on request
            if app.ui_state.request_fit {
                app.ui_state.request_fit = false;
                fit_to_geometry(app, rect.size());
            }

            // Overlay coordinate display (top-left of viewport)
            overlay_info(ui, app, rect);
            show_zoom_controls(ctx, app, rect);
            operation_status_overlay(ui, app, rect);
            if let Some(position) = crate::ui::console::simulation_position(app) {
                let world_xy = [
                    position[0] + app.project.setup.work_origin_mm[0],
                    position[1] + app.project.setup.work_origin_mm[1],
                ];
                let world_z = app.ui_state.simulator.z + app.project.setup.work_origin_mm[2];
                let screen = if app.ui_state.show_3d_view {
                    let projected = crate::renderer::pipeline::project_view_point(
                        [world_xy[0] as f32, world_xy[1] as f32, world_z as f32],
                        app.ui_state.model_orbit,
                    );
                    egui::pos2(
                        rect.center().x + (projected[0] - app.camera.pan.x) * app.camera.zoom,
                        rect.center().y - (projected[1] - app.camera.pan.y) * app.camera.zoom,
                    )
                } else {
                    egui::pos2(
                        rect.center().x + (world_xy[0] as f32 - app.camera.pan.x) * app.camera.zoom,
                        rect.center().y - (world_xy[1] as f32 - app.camera.pan.y) * app.camera.zoom,
                    )
                };
                let painter = ui.painter_at(rect);
                let tool_diameter = app
                    .ui_state
                    .selected_operation
                    .and_then(|index| app.project.operations.get(index))
                    .map(|operation| operation.tool.diameter_mm as f32)
                    .unwrap_or(app.project.default_tool.diameter_mm as f32);
                let radius = if app.ui_state.show_3d_view {
                    (tool_diameter * app.camera.zoom * 0.5).clamp(4.0, 16.0)
                } else {
                    5.0
                };
                if app.ui_state.show_3d_view {
                    let tool = app
                        .ui_state
                        .selected_operation
                        .and_then(|index| app.project.operations.get(index))
                        .map(|operation| &operation.tool)
                        .unwrap_or(&app.project.default_tool);
                    let tip = [world_xy[0] as f32, world_xy[1] as f32, world_z as f32];
                    let shaft_end = [tip[0], tip[1], tip[2] + tool.stickout_mm as f32];
                    let projected = crate::renderer::pipeline::project_view_point(
                        shaft_end,
                        app.ui_state.model_orbit,
                    );
                    let shaft_screen = egui::pos2(
                        rect.center().x + (projected[0] - app.camera.pan.x) * app.camera.zoom,
                        rect.center().y - (projected[1] - app.camera.pan.y) * app.camera.zoom,
                    );
                    painter.line_segment(
                        [screen, shaft_screen],
                        egui::Stroke::new(3.0_f32, egui::Color32::from_rgb(190, 195, 205)),
                    );
                }
                painter.circle_filled(screen, radius, egui::Color32::from_rgb(255, 75, 55));
                painter.circle_stroke(
                    screen,
                    radius + 3.0,
                    egui::Stroke::new(1.5_f32, egui::Color32::WHITE),
                );
            }

            // Paint the wgpu viewport via egui_wgpu::Callback
            let callback = egui_wgpu::Callback::new_paint_callback(
                rect,
                ViewportCallback {
                    camera: app.camera.clone(),
                    show_grid: app.ui_state.show_grid && !app.ui_state.show_3d_view,
                    viewport_size: [rect.width(), rect.height()],
                    scene: app.scene.clone(),
                },
            );
            ui.painter().add(callback);
        });
}

fn show_project_welcome(ui: &mut egui::Ui, app: &mut TekkCamApp, rect: egui::Rect) {
    let dark = app.ui_state.theme == crate::app::AppTheme::Dark;
    let panel = if dark {
        egui::Color32::from_rgb(20, 21, 29)
    } else {
        egui::Color32::from_rgb(242, 243, 249)
    };
    let border = if dark {
        egui::Color32::from_rgb(48, 50, 64)
    } else {
        egui::Color32::from_rgb(211, 213, 226)
    };
    let muted = if dark {
        egui::Color32::from_rgb(153, 158, 177)
    } else {
        egui::Color32::from_rgb(103, 107, 128)
    };
    let heading = if dark {
        egui::Color32::from_rgb(235, 237, 245)
    } else {
        egui::Color32::from_rgb(37, 39, 53)
    };
    let width = rect.width().min(700.0).max(320.0);
    let height = rect.height().min(300.0).max(250.0);
    let card_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(width, height));

    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(card_rect), |ui| {
        egui::Frame::new()
            .fill(panel)
            .stroke(egui::Stroke::new(1.0_f32, border))
            .corner_radius(egui::CornerRadius::same(14))
            .inner_margin(egui::Margin::same(30))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.vertical_centered(|ui| {
                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new("Start a project")
                            .size(30.0)
                            .strong()
                            .color(heading),
                    );
                    ui.add_space(8.0);
                    ui.add_sized(
                        [ui.available_width(), 36.0],
                        egui::Label::new(
                            egui::RichText::new(
                                "Open a TekkCAM project or create a new workspace.",
                            )
                            .size(15.0)
                            .color(muted),
                        )
                        .wrap(),
                    );

                    let button_width = ((ui.available_width() - 14.0) * 0.5).max(120.0);
                    ui.horizontal_centered(|ui| {
                        if project_action_button(
                            ui,
                            [button_width, 68.0],
                            "New project",
                            ProjectActionIcon::New,
                        )
                        .clicked()
                        {
                            app.create_new_project();
                        }
                        ui.add_space(14.0);
                        if project_action_button(
                            ui,
                            [button_width, 68.0],
                            "Open project…",
                            ProjectActionIcon::Open,
                        )
                        .clicked()
                        {
                            app.request_project_open_dialog(ui.ctx());
                        }
                    });
                });
            });
    });
}

#[derive(Clone, Copy)]
enum ProjectActionIcon {
    New,
    Open,
}

fn project_action_button(
    ui: &mut egui::Ui,
    size: [f32; 2],
    label: &str,
    icon: ProjectActionIcon,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(size[0], size[1]), egui::Sense::click());
    let visuals = ui.style().interact(&response);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, egui::CornerRadius::same(9), visuals.bg_fill);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(9),
        visuals.bg_stroke,
        egui::StrokeKind::Inside,
    );

    let label_font = egui::FontId::proportional(16.0);
    let label_galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), label_font.clone(), visuals.text_color());
    let group_width = 22.0 + 12.0 + label_galley.size().x;
    let group_left = rect.center().x - group_width * 0.5;
    let center = egui::pos2(group_left + 11.0, rect.center().y);
    let icon_stroke = egui::Stroke::new(2.0_f32, visuals.text_color());
    match icon {
        ProjectActionIcon::New => {
            painter.circle_stroke(center, 10.0, icon_stroke);
            painter.line_segment(
                [center - egui::vec2(5.0, 0.0), center + egui::vec2(5.0, 0.0)],
                icon_stroke,
            );
            painter.line_segment(
                [center - egui::vec2(0.0, 5.0), center + egui::vec2(0.0, 5.0)],
                icon_stroke,
            );
        }
        ProjectActionIcon::Open => {
            let folder = [
                egui::pos2(center.x - 11.0, center.y - 7.0),
                egui::pos2(center.x - 3.0, center.y - 7.0),
                egui::pos2(center.x, center.y - 4.0),
                egui::pos2(center.x + 10.0, center.y - 4.0),
                egui::pos2(center.x + 8.0, center.y + 8.0),
                egui::pos2(center.x - 11.0, center.y + 8.0),
            ];
            painter.add(egui::Shape::closed_line(folder.to_vec(), icon_stroke));
            painter.line_segment(
                [
                    egui::pos2(center.x - 6.0, center.y),
                    egui::pos2(center.x + 5.0, center.y),
                ],
                icon_stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(center.x + 1.0, center.y - 4.0),
                    egui::pos2(center.x + 5.0, center.y),
                ],
                icon_stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(center.x + 1.0, center.y + 4.0),
                    egui::pos2(center.x + 5.0, center.y),
                ],
                icon_stroke,
            );
        }
    }
    painter.galley(
        egui::pos2(
            group_left + 22.0 + 12.0,
            rect.center().y - label_galley.size().y * 0.5,
        ),
        label_galley,
        visuals.text_color(),
    );
    response
}

fn show_zoom_controls(ctx: &egui::Context, app: &mut TekkCamApp, rect: egui::Rect) {
    let mut zoom_by = 1.0;
    let fit_step_model = app
        .project
        .models
        .iter()
        .position(|model| model.visible && is_step_model(&model.source_path));
    egui::Area::new("viewport_zoom_controls".into())
        .fixed_pos(egui::pos2(rect.left() + 8.0, rect.top() + 30.0))
        .show(ctx, |ui| {
            ui.vertical(|ui| {
                if ui
                    .add_sized(
                        [30.0, 28.0],
                        egui::Button::new(egui::RichText::new("+").size(18.0)),
                    )
                    .on_hover_text("Zoom in")
                    .clicked()
                {
                    zoom_by = 1.2;
                }
                if ui
                    .add_sized(
                        [30.0, 28.0],
                        egui::Button::new(egui::RichText::new("−").size(18.0)),
                    )
                    .on_hover_text("Zoom out")
                    .clicked()
                {
                    zoom_by = 1.0 / 1.2;
                }
                if let Some(model_index) = fit_step_model {
                    if ui
                        .add_sized(
                            [30.0, 28.0],
                            egui::Button::new(
                                egui::RichText::new(ICON_CUBE_OUTLINE.to_string()).font(
                                    egui::FontId::new(
                                        16.0,
                                        egui::FontFamily::Name("sidebar_icons".into()),
                                    ),
                                ),
                            ),
                        )
                        .on_hover_text("Fit model to stock")
                        .clicked()
                    {
                        app.ui_state.selected_model = Some(model_index);
                        app.ui_state.fit_model_rotation_deg = app.project.models[model_index]
                            .transform
                            .rotation_degrees
                            .map(|angle| angle.round() as i16);
                        app.ui_state.show_fit_model_dialog = true;
                    }
                }
            });
        });
    if zoom_by != 1.0 {
        app.camera.zoom_at(rect.center(), zoom_by, rect);
    }
}

const ICON_CUBE_OUTLINE: char = '\u{F01A7}';

fn is_step_model(source_path: &str) -> bool {
    std::path::Path::new(source_path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "step" | "stp"))
}

fn operation_status_overlay(ui: &egui::Ui, app: &TekkCamApp, rect: egui::Rect) {
    let Some(operation) = app
        .ui_state
        .selected_operation
        .and_then(|index| app.project.operations.get(index))
    else {
        return;
    };
    let state = if operation.generated {
        "toolpath ready"
    } else {
        "not generated"
    };
    let text = format!("Operation: {} · {}", operation.name, state);
    ui.painter_at(rect).text(
        egui::pos2(rect.right() - 8.0, rect.top() + 8.0),
        egui::Align2::RIGHT_TOP,
        text,
        egui::FontId::monospace(11.0),
        egui::Color32::from_rgb(180, 195, 215),
    );
}

/// Rebuild `app.scene` when the set of visible layers changes.
fn sync_scene(app: &mut TekkCamApp, orbit_preview: bool) {
    let sig: Vec<(u64, bool, u64)> = app
        .project
        .layers
        .iter()
        .map(|l| {
            (
                l.id,
                l.visible,
                l.geometry.lines.len() as u64 + l.geometry.triangles.len() as u64,
            )
        })
        .chain(app.project.models.iter().enumerate().map(|(i, model)| {
            (
                u64::MAX - i as u64,
                model.visible,
                model.vertices.len() as u64 + model.triangles.len() as u64,
            )
        }))
        .chain(
            app.project
                .operations
                .iter()
                .enumerate()
                .map(|(i, o)| (i as u64, o.generated, o.toolpath.moves.len() as u64)),
        )
        .collect();
    let mut sig = sig;
    if let Some(operation) = app
        .ui_state
        .selected_operation
        .and_then(|index| app.project.operations.get(index))
        .filter(|operation| operation.manual_face_selection)
    {
        sig.extend(
            operation
                .source_face_ids
                .iter()
                .map(|face_id| (u64::MAX - 100, true, *face_id)),
        );
    }
    sig.push((u64::MAX, app.ui_state.show_3d_view, 0));
    sig.push((u64::MAX - 21, app.ui_state.show_stock_removal, 0));
    for (index, value) in app
        .project
        .setup
        .stock_origin_mm
        .iter()
        .chain(app.project.setup.stock_size_mm.iter())
        .enumerate()
    {
        sig.push((u64::MAX - 10 - index as u64, true, value.to_bits()));
    }
    sig.push((u64::MAX - 20, true, app.project.setup.axis as u64));
    sig.push((u64::MAX - 4, orbit_preview, 0));
    if let Some((index, transform)) = app.ui_state.fit_model_preview {
        sig.push((
            u64::MAX - 30,
            true,
            index as u64 ^ transform.scale.to_bits(),
        ));
        for (axis, value) in transform
            .rotation_degrees
            .iter()
            .chain(transform.translation_mm.iter())
            .enumerate()
        {
            sig.push((u64::MAX - 31 - axis as u64, true, value.to_bits()));
        }
    }
    sig.push((
        u64::MAX - 1,
        true,
        app.ui_state.model_orbit[0].to_bits() as u64,
    ));
    sig.push((
        u64::MAX - 2,
        true,
        app.ui_state.model_orbit[1].to_bits() as u64,
    ));
    sig.push((
        u64::MAX - 3,
        true,
        app.ui_state
            .selected_model
            .map_or(u64::MAX, |index| index as u64),
    ));
    if sig != app.scene_sig {
        app.scene_revision += 1;
        let mut models = app.project.models.clone();
        if let Some((index, transform)) = app.ui_state.fit_model_preview {
            if let Some(model) = models.get_mut(index) {
                model.transform = transform;
            }
        }
        let selected_face_ids = app
            .ui_state
            .selected_operation
            .and_then(|index| app.project.operations.get(index))
            .filter(|operation| operation.manual_face_selection)
            .map(|operation| operation.source_face_ids.as_slice())
            .unwrap_or(&[]);
        app.scene = Arc::new(Scene::build(
            &app.project.layers,
            &models,
            &app.project.operations,
            app.scene_revision,
            app.ui_state.show_3d_view,
            app.ui_state.model_orbit,
            app.ui_state.selected_model,
            selected_face_ids,
            orbit_preview,
            &app.project.setup,
            app.ui_state.show_stock_removal,
        ));
        app.scene_sig = sig;
    }
}

/// Frame the bounds of all visible layers (falls back to all layers).
fn fit_to_geometry(app: &mut TekkCamApp, size: egui::Vec2) {
    if app.ui_state.show_3d_view {
        if let Some(model) = app
            .ui_state
            .selected_model
            .and_then(|index| app.project.models.get(index))
            .filter(|model| model.visible)
        {
            let (min, max) =
                crate::renderer::pipeline::projected_model_bounds(model, app.ui_state.model_orbit);
            app.camera.fit_bounds(min, max, size);
            return;
        }
        if let Some((mn, mx)) = app.scene.bounds() {
            app.camera.fit_bounds(mn, mx, size);
            return;
        }
    }
    let mut bounds: Option<([f32; 2], [f32; 2])> = None;
    for l in app.project.layers.iter().filter(|l| l.visible) {
        if let Some((mn, mx)) = l.geometry.bounds() {
            bounds = Some(match bounds {
                None => (mn, mx),
                Some((a, b)) => (
                    [a[0].min(mn[0]), a[1].min(mn[1])],
                    [b[0].max(mx[0]), b[1].max(mx[1])],
                ),
            });
        }
    }
    if app.ui_state.show_3d_view {
        for model in app.project.models.iter().filter(|model| model.visible) {
            let model_bounds = model.transformed_bounds();
            let mn = [model_bounds[0][0], model_bounds[0][1]];
            let mx = [model_bounds[1][0], model_bounds[1][1]];
            bounds = Some(match bounds {
                None => (mn, mx),
                Some((a, b)) => (
                    [a[0].min(mn[0]), a[1].min(mn[1])],
                    [b[0].max(mx[0]), b[1].max(mx[1])],
                ),
            });
        }
    }
    if let Some((mn, mx)) = bounds {
        app.camera.fit_bounds(mn, mx, size);
    }
}

/// Handle mouse pan and scroll-wheel zoom in the viewport.
fn handle_camera_input(ui: &mut egui::Ui, app: &mut TekkCamApp, rect: egui::Rect) -> bool {
    if app.ui_state.face_selection_mode {
        let selection_is_valid = app.ui_state.show_3d_view
            && app.project.models.iter().any(|model| model.visible)
            && app
                .ui_state
                .selected_operation
                .and_then(|index| app.project.operations.get(index))
                .is_some_and(|operation| {
                    operation.manual_face_selection
                        && matches!(
                            &operation.kind,
                            crate::cam::operations::OperationKind::ModelProfile2D
                        )
                });
        if !selection_is_valid {
            app.ui_state.face_selection_mode = false;
        }
    }
    let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());

    if app.ui_state.face_selection_mode && response.clicked_by(egui::PointerButton::Primary) {
        if let Some(pointer_pos) = response.interact_pointer_pos() {
            toggle_model_face_at(app, pointer_pos, rect);
        }
    }

    if response.double_clicked() {
        if let Some(pointer_pos) = response.interact_pointer_pos() {
            app.camera.zoom_at(pointer_pos, 2.0, rect);
        }
    }

    // Pan (middle-mouse drag OR left drag with no selection intent)
    if response.dragged_by(egui::PointerButton::Middle)
        || (!app.ui_state.face_selection_mode && response.dragged_by(egui::PointerButton::Primary))
    {
        let delta = response.drag_delta();
        app.camera.pan_pixels(delta);
    }
    let orbit_preview =
        app.ui_state.show_3d_view && response.dragged_by(egui::PointerButton::Secondary);
    if orbit_preview {
        let delta = response.drag_delta();
        app.ui_state.model_orbit[0] += delta.x * 0.008;
        app.ui_state.model_orbit[1] =
            (app.ui_state.model_orbit[1] + delta.y * 0.008).clamp(-0.565, 0.905);
    }

    // Zoom (scroll wheel) — only while the pointer is over the viewport
    let scroll = ui.input(|i| i.smooth_scroll_delta.y);
    if scroll != 0.0 {
        let zoom_factor = if scroll > 0.0 { 1.1f32 } else { 1.0 / 1.1 };
        if let Some(pointer_pos) = ui.input(|i| i.pointer.hover_pos()) {
            if rect.contains(pointer_pos) {
                app.camera.zoom_at(pointer_pos, zoom_factor, rect);
            }
        }
    }
    orbit_preview
}

fn toggle_model_face_at(app: &mut TekkCamApp, pointer: egui::Pos2, rect: egui::Rect) {
    let Some(operation_index) = app
        .ui_state
        .selected_operation
        .filter(|&index| index < app.project.operations.len())
    else {
        app.ui_state.status_message = "Select a STEP face-profile operation first".into();
        return;
    };

    let model_index = app
        .ui_state
        .selected_model
        .filter(|&index| {
            app.project
                .models
                .get(index)
                .is_some_and(|model| model.visible)
        })
        .or_else(|| app.project.models.iter().position(|model| model.visible));
    let Some(model_index) = model_index else {
        app.ui_state.status_message = "No visible STEP model to select faces from".into();
        return;
    };
    let model = &app.project.models[model_index];
    let mut best: Option<(f32, u64)> = None;
    for body in &model.bodies {
        for offset in 0..body.triangle_count {
            let triangle_index = body.first_triangle + offset;
            let Some(triangle) = model.triangles.get(triangle_index) else {
                continue;
            };
            let face_id = body
                .triangle_face_ids
                .get(offset)
                .copied()
                .unwrap_or(body.source_item_id);
            if face_id == 0 {
                continue;
            }
            let Some(points) = triangle
                .map(|vertex| model.transformed_vertex(vertex as usize))
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .and_then(|points| {
                    <Vec<[f32; 3]> as TryInto<[[f32; 3]; 3]>>::try_into(points).ok()
                })
            else {
                continue;
            };
            if !crate::renderer::pipeline::view_triangle_is_front_facing(
                points,
                app.ui_state.model_orbit,
            ) {
                continue;
            }

            let screen_points = points.map(|point| {
                let projected =
                    crate::renderer::pipeline::project_view_point(point, app.ui_state.model_orbit);
                egui::pos2(
                    rect.center().x + (projected[0] - app.camera.pan.x) * app.camera.zoom,
                    rect.center().y - (projected[1] - app.camera.pan.y) * app.camera.zoom,
                )
            });
            let Some(weights) = screen_barycentric(pointer, screen_points) else {
                continue;
            };
            let depth = points
                .iter()
                .zip(weights)
                .map(|(point, weight)| {
                    crate::renderer::pipeline::view_depth(*point, app.ui_state.model_orbit) * weight
                })
                .sum::<f32>();
            if best.is_none_or(|(closest, _)| depth < closest) {
                best = Some((depth, face_id));
            }
        }
    }

    let Some((_, face_id)) = best else {
        app.ui_state.status_message = "No model face at that position".into();
        return;
    };
    let operation = &mut app.project.operations[operation_index];
    operation.manual_face_selection = true;
    if operation.source_face_ids.contains(&face_id) {
        operation
            .source_face_ids
            .retain(|selected| *selected != face_id);
        app.ui_state.status_message = format!("Deselected face {face_id}");
    } else {
        operation.source_face_ids.push(face_id);
        app.ui_state.status_message = format!("Selected face {face_id}");
    }
    app.ui_state.project_dirty = true;
}

fn screen_barycentric(point: egui::Pos2, triangle: [egui::Pos2; 3]) -> Option<[f32; 3]> {
    let [a, b, c] = triangle;
    let denominator = (b.y - c.y) * (a.x - c.x) + (c.x - b.x) * (a.y - c.y);
    if denominator.abs() < 1e-5 {
        return None;
    }
    let first = ((b.y - c.y) * (point.x - c.x) + (c.x - b.x) * (point.y - c.y)) / denominator;
    let second = ((c.y - a.y) * (point.x - c.x) + (a.x - c.x) * (point.y - c.y)) / denominator;
    let third = 1.0 - first - second;
    (first >= -1e-4 && second >= -1e-4 && third >= -1e-4).then_some([first, second, third])
}

/// Draw an overlay with the cursor world position, zoom level, and viewport hints.
fn overlay_info(ui: &mut egui::Ui, app: &mut TekkCamApp, rect: egui::Rect) {
    let painter = ui.painter_at(rect);

    let cursor_world = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|p| rect.contains(*p))
        .map(|screen_pos| app.camera.screen_to_world(screen_pos, rect));

    let zoom_level = app.camera.zoom;
    let units = app.ui_state.display_units;
    let factor = units.factor() as f32;

    let coord_text = if app.ui_state.show_3d_view {
        format!("3D orthographic view  ·  Zoom: {:.2}×", zoom_level)
    } else if let Some(pos) = cursor_world {
        format!(
            "X: {:.3}  Y: {:.3} {}   Zoom: {:.2}×",
            pos.x / factor,
            pos.y / factor,
            units.label(),
            zoom_level
        )
    } else {
        format!("Zoom: {:.2}× · {}", zoom_level, units.label())
    };

    let font = egui::FontId::monospace(11.0);
    let text_pos = egui::pos2(rect.left() + 8.0, rect.top() + 8.0);

    // Shadow
    painter.text(
        text_pos + egui::vec2(1.0, 1.0),
        egui::Align2::LEFT_TOP,
        &coord_text,
        font.clone(),
        egui::Color32::from_black_alpha(180),
    );
    // Text
    painter.text(
        text_pos,
        egui::Align2::LEFT_TOP,
        &coord_text,
        font,
        egui::Color32::from_rgb(160, 200, 160),
    );

    // Hint text (bottom-right)
    let hint = if app.ui_state.show_3d_view {
        "Scroll: zoom  |  Double-click: zoom in  |  Drag: pan  |  Right drag: orbit"
    } else {
        "Scroll: zoom  |  Double-click: zoom in  |  Drag: pan"
    };
    let hint_pos = egui::pos2(rect.right() - 8.0, rect.bottom() - 8.0);
    painter.text(
        hint_pos,
        egui::Align2::RIGHT_BOTTOM,
        hint,
        egui::FontId::proportional(10.0),
        egui::Color32::from_rgba_premultiplied(120, 120, 140, 160),
    );
}
