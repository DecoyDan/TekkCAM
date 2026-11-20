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

            show_stock_render_controls(ctx, app, rect);

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

            // Keep viewport decorations and the simulated cutter above rendered stock.
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

            show_text_placement_outline(ui, app, rect);
            show_text_placement_prompt(ctx, app, rect);
            show_simulation_dro(ctx, ui, app, rect);
            if app.ui_state.show_3d_view {
                show_view_cube(ui, app, rect);
            }
            if !app.ui_state.simulation_moves.is_empty() {
                show_simulation_controls(ctx, app);
            }
        });
}

fn show_simulation_controls(ctx: &egui::Context, app: &mut TekkCamApp) {
    let move_count = app.ui_state.simulation_moves.len();
    let completed =
        app.ui_state.simulator.move_index as f32 + app.ui_state.simulator.fraction as f32;
    let mut progress = (completed / move_count as f32).clamp(0.0, 1.0);
    egui::Area::new(egui::Id::new("simulation_playback_controls"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -50.0))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_premultiplied(18, 22, 30, 238))
                .stroke(egui::Stroke::new(
                    1.0_f32,
                    egui::Color32::from_rgb(65, 83, 108),
                ))
                .corner_radius(egui::CornerRadius::same(7))
                .inner_margin(egui::Margin::symmetric(10, 7))
                .show(ui, |ui| {
                    ui.set_min_width(720.0);
                    ui.horizontal(|ui| {
                        let play_pause = ui
                            .add_sized(
                                [30.0, 24.0],
                                egui::Button::new(if app.ui_state.simulator.running {
                                    "⏸"
                                } else {
                                    "▶"
                                }),
                            )
                            .on_hover_text(if app.ui_state.simulator.running {
                                "Pause simulation"
                            } else {
                                "Play simulation"
                            });
                        if play_pause.clicked() {
                            if app.ui_state.simulator.running {
                                app.ui_state.simulator.stop();
                            } else if app.ui_state.simulator.move_index
                                >= app.ui_state.simulation_moves.len()
                            {
                                app.ui_state.simulator.reset();
                            } else {
                                app.ui_state.simulator.running = true;
                            }
                            ctx.request_repaint();
                        }
                        let default_slider_width = ui.spacing().slider_width;
                        ui.spacing_mut().slider_width = 480.0;
                        let slider = ui.add_sized(
                            [480.0, 18.0],
                            egui::Slider::new(&mut progress, 0.0..=1.0).show_value(false),
                        );
                        ui.spacing_mut().slider_width = default_slider_width;
                        if slider.changed() {
                            app.ui_state
                                .simulator
                                .seek(&app.ui_state.simulation_moves, progress);
                        }
                        ui.label(format!("{:.0}%", progress * 100.0));
                        ui.separator();
                        for speed in [1.0_f32, 2.5, 5.0, 10.0] {
                            ui.selectable_value(
                                &mut app.ui_state.simulation_speed,
                                speed,
                                format!("{speed}×"),
                            );
                        }
                    });
                });
        });
}

fn show_stock_render_controls(ctx: &egui::Context, app: &mut TekkCamApp, viewport: egui::Rect) {
    egui::Area::new(egui::Id::new("stock_render_controls"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(viewport.left() + 12.0, viewport.bottom() - 12.0))
        .pivot(egui::Align2::LEFT_BOTTOM)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_premultiplied(18, 22, 30, 225))
                .stroke(egui::Stroke::new(
                    1.0_f32,
                    egui::Color32::from_rgb(65, 83, 108),
                ))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(7, 5))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(!app.ui_state.show_3d_view, "2D")
                            .clicked()
                        {
                            app.ui_state.show_3d_view = false;
                            app.ui_state.render_stock = false;
                            app.ui_state.request_fit = true;
                        }
                        if ui
                            .selectable_label(
                                app.ui_state.show_3d_view && !app.ui_state.render_stock,
                                "3D",
                            )
                            .clicked()
                        {
                            app.ui_state.show_3d_view = true;
                            app.ui_state.render_stock = false;
                            app.ui_state.show_stock_removal = false;
                            app.ui_state.model_orbit = [0.0; 2];
                            app.ui_state.request_fit = true;
                        }
                        if ui
                            .selectable_label(app.ui_state.render_stock, "Render")
                            .clicked()
                        {
                            app.ui_state.show_3d_view = true;
                            app.ui_state.render_stock = true;
                            app.ui_state.show_stock_removal = true;
                        }
                    });
                });
        });
}

fn show_simulation_dro(ctx: &egui::Context, ui: &egui::Ui, app: &TekkCamApp, viewport: egui::Rect) {
    let visible = !app.ui_state.simulation_moves.is_empty();
    let animation = ctx.animate_value_with_time(
        egui::Id::new("simulation_dro_slide"),
        if visible { 1.0 } else { 0.0 },
        0.22,
    );
    if animation <= 0.01 {
        return;
    }
    let Some(position) = app.ui_state.simulator.position else {
        return;
    };
    let units = app.ui_state.display_units;
    let factor = units.factor();
    let origin = app.project.setup.work_origin_mm;
    let values = [
        ("X", (position[0] + origin[0]) / factor),
        ("Y", (position[1] + origin[1]) / factor),
        ("Z", (app.ui_state.simulator.z + origin[2]) / factor),
    ];
    let rect = egui::Rect::from_min_size(
        egui::pos2(
            viewport.left() + 8.0,
            viewport.top() + 30.0 - (1.0 - animation) * 58.0,
        ),
        egui::vec2(300.0, 52.0),
    );
    let painter = ui.painter_at(viewport);
    painter.rect_filled(
        rect,
        egui::CornerRadius::same(6),
        egui::Color32::from_rgba_premultiplied(
            (18.0 * animation) as u8,
            (22.0 * animation) as u8,
            (30.0 * animation) as u8,
            (230.0 * animation) as u8,
        ),
    );
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(6),
        egui::Stroke::new(
            1.0_f32,
            egui::Color32::from_rgba_unmultiplied(65, 83, 108, (255.0 * animation) as u8),
        ),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.left_top() + egui::vec2(8.0, 5.0),
        egui::Align2::LEFT_TOP,
        "SIMULATION DRO",
        egui::FontId::proportional(9.0),
        egui::Color32::from_rgba_unmultiplied(115, 205, 145, (255.0 * animation) as u8),
    );
    for (index, (axis, value)) in values.iter().enumerate() {
        let x = rect.left() + 8.0 + index as f32 * 96.0;
        let y = rect.top() + 24.0;
        painter.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_CENTER,
            format!("{axis} {value:>8.3}{}", units.label()),
            egui::FontId::monospace(10.5),
            egui::Color32::from_rgba_unmultiplied(225, 232, 242, (255.0 * animation) as u8),
        );
    }
}

fn show_view_cube(ui: &mut egui::Ui, app: &mut TekkCamApp, viewport: egui::Rect) {
    let side = 30.0;
    let scale = 22.0;
    let center = egui::pos2(viewport.right() - 48.0, viewport.top() + 70.0);
    let yaw = std::f32::consts::FRAC_PI_4 + app.ui_state.model_orbit[0];
    let pitch = (35.264_f32.to_radians() + app.ui_state.model_orbit[1]).clamp(-1.52, 1.52);
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let project = |point: [f32; 3]| {
        let x = point[0] * cy - point[1] * sy;
        let y = point[0] * sy + point[1] * cy;
        egui::pos2(
            center.x + x * scale,
            center.y - (y * sp + point[2] * cp) * scale,
        )
    };
    let depth = |point: [f32; 3]| {
        let y = point[0] * sy + point[1] * cy;
        y * cp - point[2] * sp
    };
    let faces: [(&str, [f32; 3], [[f32; 3]; 4], egui::Color32, [f32; 3]); 6] = [
        (
            "Top",
            [0.0, 0.0, 1.0],
            [
                [-1.0, -1.0, 1.0],
                [1.0, -1.0, 1.0],
                [1.0, 1.0, 1.0],
                [-1.0, 1.0, 1.0],
            ],
            egui::Color32::from_rgb(83, 112, 156),
            [0.0, 0.0, 1.0],
        ),
        (
            "Bottom",
            [0.0, 0.0, -1.0],
            [
                [-1.0, 1.0, -1.0],
                [1.0, 1.0, -1.0],
                [1.0, -1.0, -1.0],
                [-1.0, -1.0, -1.0],
            ],
            egui::Color32::from_rgb(58, 68, 86),
            [0.0, 0.0, -1.0],
        ),
        (
            "Front",
            [0.0, 1.0, 0.0],
            [
                [-1.0, 1.0, -1.0],
                [1.0, 1.0, -1.0],
                [1.0, 1.0, 1.0],
                [-1.0, 1.0, 1.0],
            ],
            egui::Color32::from_rgb(71, 128, 190),
            [0.0, 1.0, 0.0],
        ),
        (
            "Back",
            [0.0, -1.0, 0.0],
            [
                [1.0, -1.0, -1.0],
                [-1.0, -1.0, -1.0],
                [-1.0, -1.0, 1.0],
                [1.0, -1.0, 1.0],
            ],
            egui::Color32::from_rgb(55, 91, 132),
            [0.0, -1.0, 0.0],
        ),
        (
            "Right",
            [1.0, 0.0, 0.0],
            [
                [1.0, 1.0, -1.0],
                [1.0, -1.0, -1.0],
                [1.0, -1.0, 1.0],
                [1.0, 1.0, 1.0],
            ],
            egui::Color32::from_rgb(90, 151, 198),
            [1.0, 0.0, 0.0],
        ),
        (
            "Left",
            [-1.0, 0.0, 0.0],
            [
                [-1.0, -1.0, -1.0],
                [-1.0, 1.0, -1.0],
                [-1.0, 1.0, 1.0],
                [-1.0, -1.0, 1.0],
            ],
            egui::Color32::from_rgb(62, 106, 151),
            [-1.0, 0.0, 0.0],
        ),
    ];
    let toward_camera = [sy * cp, cy * cp, -sp];
    let mut visible: Vec<_> = faces
        .iter()
        .filter(|(_, normal, _, _, _)| {
            normal[0] * toward_camera[0]
                + normal[1] * toward_camera[1]
                + normal[2] * toward_camera[2]
                > 0.001
        })
        .collect();
    visible.sort_by(|a, b| {
        let avg_depth = |face: &&(&str, [f32; 3], [[f32; 3]; 4], egui::Color32, [f32; 3])| {
            face.2.iter().map(|point| depth(*point)).sum::<f32>() / 4.0
        };
        avg_depth(a).total_cmp(&avg_depth(b))
    });
    let cube_rect = egui::Rect::from_center_size(center, egui::vec2(side * 2.4, side * 2.4));
    let response = ui.interact(
        cube_rect,
        egui::Id::new("viewport_view_cube"),
        egui::Sense::click(),
    );
    let painter = ui.painter_at(viewport);
    for (label, _, vertices, color, normal) in &visible {
        let points: Vec<_> = vertices.iter().map(|point| project(*point)).collect();
        let face_center = project(*normal);
        painter.add(egui::Shape::convex_polygon(
            points,
            *color,
            egui::Stroke::new(1.2_f32, egui::Color32::from_rgb(205, 218, 238)),
        ));
        painter.text(
            face_center,
            egui::Align2::CENTER_CENTER,
            *label,
            egui::FontId::proportional(8.5),
            egui::Color32::WHITE,
        );
    }
    if response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            if let Some((label, ..)) = visible.iter().min_by(|a, b| {
                let da = project(a.4).distance_sq(pointer);
                let db = project(b.4).distance_sq(pointer);
                da.total_cmp(&db)
            }) {
                let flat_pitch = -0.565;
                app.ui_state.model_orbit = match *label {
                    "Top" => [0.0, 0.905],
                    "Bottom" => [0.0, -2.135],
                    "Front" => [-std::f32::consts::FRAC_PI_4, flat_pitch],
                    "Right" => [std::f32::consts::FRAC_PI_4, flat_pitch],
                    "Back" => [3.0 * std::f32::consts::FRAC_PI_4, flat_pitch],
                    "Left" => [-3.0 * std::f32::consts::FRAC_PI_4, flat_pitch],
                    _ => [0.0, 0.0],
                };
                app.ui_state.request_fit = true;
            }
        }
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
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
    let dro_visible = !app.ui_state.simulation_moves.is_empty();
    let dro_offset = ctx.animate_value_with_time(
        egui::Id::new("zoom_controls_below_dro"),
        if dro_visible { 58.0 } else { 0.0 },
        0.22,
    );
    let fit_step_model = app
        .project
        .models
        .iter()
        .position(|model| model.visible && is_step_model(&model.source_path));
    egui::Area::new("viewport_zoom_controls".into())
        .fixed_pos(egui::pos2(
            rect.left() + 8.0,
            rect.top() + 30.0 + dro_offset,
        ))
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
    sig.push((u64::MAX - 22, app.ui_state.render_stock, 0));
    sig.push((u64::MAX - 23, true, app.project.stock_material as u64));
    if !app.ui_state.simulation_moves.is_empty() {
        sig.push((
            u64::MAX - 24,
            true,
            app.ui_state.simulator.move_index as u64,
        ));
        sig.push((
            u64::MAX - 25,
            true,
            app.ui_state.simulator.fraction.to_bits(),
        ));
    }
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
        let simulation = (!app.ui_state.simulation_moves.is_empty()).then_some(
            crate::renderer::pipeline::SimulationProgress {
                moves: &app.ui_state.simulation_moves,
                state: &app.ui_state.simulator,
            },
        );
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
            app.ui_state.render_stock,
            app.project.stock_material,
            simulation,
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

    let placing_text = app
        .ui_state
        .placing_text_layer
        .filter(|&index| app.project.layers.get(index).is_some());
    let primary_down = ui.input(|input| input.pointer.button_down(egui::PointerButton::Primary));
    if primary_down || response.drag_started_by(egui::PointerButton::Primary) {
        if let (Some(index), Some(pointer)) = (
            app.ui_state
                .placing_text_layer
                .or(app.ui_state.selected_layer)
                .filter(|&index| {
                    app.project.layers.get(index).is_some_and(|layer| {
                        layer.visible && layer.kind == crate::workspace::layer::LayerKind::Text
                    })
                }),
            response.interact_pointer_pos(),
        ) {
            if let Some(state) = text_rotate_state_at_pointer(app, index, pointer, rect) {
                app.ui_state.text_rotate = Some(state);
            } else if let Some(state) = text_resize_state_at_pointer(app, index, pointer, rect) {
                app.ui_state.text_resize = Some(state);
            }
        }
    }
    if let Some(rotate) = app.ui_state.text_rotate.clone() {
        if response.dragged_by(egui::PointerButton::Primary) {
            if let Some(pointer) = response.interact_pointer_pos() {
                let cursor = screen_to_text_world(app, pointer, rect);
                rotate_text_layer(app, &rotate, [cursor.x, cursor.y]);
            }
        }
        if response.drag_stopped_by(egui::PointerButton::Primary) {
            app.ui_state.text_rotate = None;
            app.ui_state.project_dirty = true;
            app.ui_state.status_message = "Text rotated".into();
        }
    }
    if let Some(resize) = app.ui_state.text_resize.clone() {
        if response.dragged_by(egui::PointerButton::Primary) {
            if let Some(pointer) = response.interact_pointer_pos() {
                let cursor = screen_to_text_world(app, pointer, rect);
                resize_text_layer(app, &resize, [cursor.x, cursor.y]);
            }
        }
        if response.drag_stopped_by(egui::PointerButton::Primary) {
            app.ui_state.text_resize = None;
            app.ui_state.project_dirty = true;
            app.ui_state.status_message = "Text resized".into();
        }
    }
    if app.ui_state.text_resize.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
    } else if app.ui_state.text_rotate.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if let (Some(index), Some(pointer)) = (
        app.ui_state.placing_text_layer,
        ui.input(|input| input.pointer.hover_pos()),
    ) {
        if text_resize_state_at_pointer(app, index, pointer, rect).is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
        }
    }
    let mut text_double_click = false;
    if !app.ui_state.face_selection_mode {
        if let Some(index) = placing_text {
            if app.ui_state.text_rotate.is_none() && app.ui_state.text_resize.is_none() {
                if let Some(pointer_pos) = response.interact_pointer_pos() {
                    let cursor = screen_to_text_world(app, pointer_pos, rect);
                    move_text_layer_to_cursor(app, index, cursor);
                }
            }
            if response.double_clicked() {
                app.ui_state.placing_text_layer = None;
                app.ui_state.project_dirty = true;
                app.ui_state.status_message = "Design placed".into();
                text_double_click = true;
            }
        } else if response.double_clicked() {
            if let Some(pointer_pos) = response.interact_pointer_pos() {
                let cursor = screen_to_text_world(app, pointer_pos, rect);
                if let Some(index) = movable_layer_at_world(app, cursor) {
                    app.ui_state.selected_layer = Some(index);
                    app.ui_state.placing_text_layer = Some(index);
                    app.ui_state.text_placement_offset = layer_geometry_center(app, index)
                        .map(|center| [cursor.x as f64 - center[0], cursor.y as f64 - center[1]])
                        .unwrap_or([0.0; 2]);
                    app.ui_state.status_message = if app.project.layers[index].kind
                        == crate::workspace::layer::LayerKind::Drawing
                    {
                        "Move design and double-click to place it".into()
                    } else {
                        "Move text and double-click to place it".into()
                    };
                    text_double_click = true;
                }
            }
        }
    }

    // Double-clicking text starts placement; double-clicking during placement
    // commits it. Keep ordinary double-click zoom for all other viewport areas.
    if response.double_clicked() && !text_double_click && placing_text.is_none() {
        if let Some(pointer_pos) = response.interact_pointer_pos() {
            app.camera.zoom_at(pointer_pos, 2.0, rect);
        }
    }

    // Pan with middle mouse, or primary drag when text placement is inactive.
    if response.dragged_by(egui::PointerButton::Middle)
        || (placing_text.is_none()
            && app.ui_state.placing_text_layer.is_none()
            && app.ui_state.text_resize.is_none()
            && !app.ui_state.face_selection_mode
            && response.dragged_by(egui::PointerButton::Primary))
    {
        let delta = response.drag_delta();
        app.camera.pan_pixels(delta);
    }
    let orbit_preview =
        app.ui_state.show_3d_view && response.dragged_by(egui::PointerButton::Secondary);
    if orbit_preview {
        let delta = response.drag_delta();
        app.ui_state.model_orbit[0] += delta.x * 0.008;
        app.ui_state.model_orbit[1] = (app.ui_state.model_orbit[1] + delta.y * 0.008).clamp(
            -1.52 - 35.264_f32.to_radians(),
            1.52 - 35.264_f32.to_radians(),
        );
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

fn layer_geometry_center(app: &TekkCamApp, index: usize) -> Option<[f64; 2]> {
    let (min, max) = app.project.layers.get(index)?.geometry.bounds()?;
    Some([
        (min[0] as f64 + max[0] as f64) * 0.5,
        (min[1] as f64 + max[1] as f64) * 0.5,
    ])
}

fn text_surface_z(app: &TekkCamApp) -> f32 {
    match app.project.setup.axis {
        crate::workspace::setup::WorkAxis::ZUp => {
            (app.project.setup.stock_origin_mm[2] + app.project.setup.stock_size_mm[2]) as f32
        }
        crate::workspace::setup::WorkAxis::ZDown => app.project.setup.stock_origin_mm[2] as f32,
    }
}

fn screen_to_text_world(app: &TekkCamApp, pointer: egui::Pos2, viewport: egui::Rect) -> glam::Vec2 {
    if !app.ui_state.show_3d_view {
        return app.camera.screen_to_world(pointer, viewport);
    }
    let projected_x = (pointer.x - viewport.center().x) / app.camera.zoom + app.camera.pan.x;
    let projected_y = -(pointer.y - viewport.center().y) / app.camera.zoom + app.camera.pan.y;
    let yaw = std::f32::consts::FRAC_PI_4 + app.ui_state.model_orbit[0];
    let pitch = (35.264_f32.to_radians() + app.ui_state.model_orbit[1]).clamp(-1.52, 1.52);
    let (sin_yaw, cos_yaw) = yaw.sin_cos();
    let (sin_pitch, cos_pitch) = pitch.sin_cos();
    let z = text_surface_z(app);
    let rotated_y = if sin_pitch.abs() < 0.05 {
        projected_y
    } else {
        (projected_y - z * cos_pitch) / sin_pitch
    };
    glam::Vec2::new(
        projected_x * cos_yaw + rotated_y * sin_yaw,
        -projected_x * sin_yaw + rotated_y * cos_yaw,
    )
}

fn text_world_to_screen(app: &TekkCamApp, point: [f32; 2], viewport: egui::Rect) -> egui::Pos2 {
    if app.ui_state.show_3d_view {
        let projected = crate::renderer::pipeline::project_view_point(
            [point[0], point[1], text_surface_z(app)],
            app.ui_state.model_orbit,
        );
        egui::pos2(
            viewport.center().x + (projected[0] - app.camera.pan.x) * app.camera.zoom,
            viewport.center().y - (projected[1] - app.camera.pan.y) * app.camera.zoom,
        )
    } else {
        egui::pos2(
            viewport.center().x + (point[0] - app.camera.pan.x) * app.camera.zoom,
            viewport.center().y - (point[1] - app.camera.pan.y) * app.camera.zoom,
        )
    }
}

fn movable_layer_at_world(app: &TekkCamApp, point: glam::Vec2) -> Option<usize> {
    let tolerance = 8.0 / app.camera.zoom.max(0.05);
    app.project
        .layers
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, layer)| {
            if !layer.visible
                || !matches!(
                    layer.kind,
                    crate::workspace::layer::LayerKind::Text
                        | crate::workspace::layer::LayerKind::Drawing
                )
            {
                return None;
            }
            let (min, max) = layer.geometry.bounds()?;
            (point.x >= min[0] - tolerance
                && point.x <= max[0] + tolerance
                && point.y >= min[1] - tolerance
                && point.y <= max[1] + tolerance)
                .then_some(index)
        })
}

fn move_text_layer_to_cursor(app: &mut TekkCamApp, index: usize, cursor: glam::Vec2) {
    let Some(center) = layer_geometry_center(app, index) else {
        return;
    };
    let target = [
        cursor.x as f64 - app.ui_state.text_placement_offset[0],
        cursor.y as f64 - app.ui_state.text_placement_offset[1],
    ];
    let delta = [target[0] - center[0], target[1] - center[1]];
    if delta[0].abs() < 1e-9 && delta[1].abs() < 1e-9 {
        return;
    }
    let geometry = &mut app.project.layers[index].geometry;
    for point in geometry
        .lines
        .iter_mut()
        .chain(geometry.triangles.iter_mut())
    {
        point[0] += delta[0] as f32;
        point[1] += delta[1] as f32;
    }
    app.scene_sig.clear();
}

fn text_resize_state_at_pointer(
    app: &TekkCamApp,
    index: usize,
    pointer: egui::Pos2,
    viewport: egui::Rect,
) -> Option<crate::app::TextResizeState> {
    let layer = app.project.layers.get(index)?;
    let (min, max) = layer.geometry.bounds()?;
    let handle_world = [max[0], min[1]];
    let handle_screen = text_world_to_screen(app, handle_world, viewport);
    if handle_screen.distance(pointer) > 10.0 {
        return None;
    }
    Some(crate::app::TextResizeState {
        layer_index: index,
        original_geometry: layer.geometry.clone(),
        anchor: [min[0], max[1]],
        initial_handle: handle_world,
        original_height_mm: layer.text_data.as_ref().map(|data| data.height_mm),
    })
}

fn text_rotate_state_at_pointer(
    app: &TekkCamApp,
    index: usize,
    pointer: egui::Pos2,
    viewport: egui::Rect,
) -> Option<crate::app::TextRotateState> {
    let layer = app.project.layers.get(index)?;
    let (min, max) = layer.geometry.bounds()?;
    let handle = text_world_to_screen(app, [(min[0] + max[0]) * 0.5, max[1]], viewport)
        + egui::vec2(0.0, -22.0);
    if handle.distance(pointer) > 10.0 {
        return None;
    }
    let center = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
    let cursor = screen_to_text_world(app, pointer, viewport);
    Some(crate::app::TextRotateState {
        layer_index: index,
        original_geometry: layer.geometry.clone(),
        center,
        initial_angle: (cursor.y - center[1]).atan2(cursor.x - center[0]),
        initial_rotation_degrees: layer
            .text_data
            .as_ref()
            .map_or(0.0, |data| data.rotation_degrees),
    })
}

fn rotate_text_layer(app: &mut TekkCamApp, rotate: &crate::app::TextRotateState, cursor: [f32; 2]) {
    let angle =
        (cursor[1] - rotate.center[1]).atan2(cursor[0] - rotate.center[0]) - rotate.initial_angle;
    let (sin, cos) = angle.sin_cos();
    let layer = &mut app.project.layers[rotate.layer_index];
    layer.geometry = rotate.original_geometry.clone();
    for point in layer
        .geometry
        .lines
        .iter_mut()
        .chain(layer.geometry.triangles.iter_mut())
    {
        let x = point[0] - rotate.center[0];
        let y = point[1] - rotate.center[1];
        point[0] = rotate.center[0] + x * cos - y * sin;
        point[1] = rotate.center[1] + x * sin + y * cos;
    }
    if let Some(data) = &mut layer.text_data {
        data.rotation_degrees = rotate.initial_rotation_degrees + angle.to_degrees() as f64;
    }
    app.scene_sig.clear();
}

fn resize_text_layer(app: &mut TekkCamApp, resize: &crate::app::TextResizeState, cursor: [f32; 2]) {
    let initial = [
        resize.initial_handle[0] - resize.anchor[0],
        resize.initial_handle[1] - resize.anchor[1],
    ];
    let current = [cursor[0] - resize.anchor[0], cursor[1] - resize.anchor[1]];
    let initial_length = initial[0].hypot(initial[1]);
    if initial_length <= f32::EPSILON {
        return;
    }
    let scale = (current[0].hypot(current[1]) / initial_length).clamp(0.15, 20.0);
    let layer = &mut app.project.layers[resize.layer_index];
    layer.geometry = resize.original_geometry.clone();
    for point in layer
        .geometry
        .lines
        .iter_mut()
        .chain(layer.geometry.triangles.iter_mut())
    {
        point[0] = resize.anchor[0] + (point[0] - resize.anchor[0]) * scale;
        point[1] = resize.anchor[1] + (point[1] - resize.anchor[1]) * scale;
    }
    if let (Some(data), Some(original_height)) = (&mut layer.text_data, resize.original_height_mm) {
        data.height_mm = original_height * scale as f64;
    }
    app.scene_sig.clear();
}

fn show_text_placement_prompt(ctx: &egui::Context, app: &TekkCamApp, viewport: egui::Rect) {
    if app.ui_state.placing_text_layer.is_none() {
        return;
    }
    let prompt = "Move design with cursor · Double-click to place";
    egui::Area::new(egui::Id::new("text_placement_prompt"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(viewport.center().x, viewport.bottom() - 16.0))
        .pivot(egui::Align2::CENTER_BOTTOM)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_premultiplied(18, 22, 30, 238))
                .stroke(egui::Stroke::new(
                    1.0_f32,
                    egui::Color32::from_rgb(65, 83, 108),
                ))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(12, 7))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(prompt).color(egui::Color32::WHITE));
                });
        });
}

fn show_text_placement_outline(ui: &egui::Ui, app: &mut TekkCamApp, viewport: egui::Rect) {
    let Some(index) = app.ui_state.placing_text_layer else {
        return;
    };
    let Some((min, max)) = app
        .project
        .layers
        .get(index)
        .filter(|layer| layer.visible)
        .and_then(|layer| layer.geometry.bounds())
    else {
        return;
    };
    let projected_corners = [
        text_world_to_screen(app, min, viewport),
        text_world_to_screen(app, [min[0], max[1]], viewport),
        text_world_to_screen(app, [max[0], min[1]], viewport),
        text_world_to_screen(app, max, viewport),
    ];
    let screen_min = projected_corners
        .iter()
        .fold(egui::pos2(f32::INFINITY, f32::INFINITY), |min, point| {
            egui::pos2(min.x.min(point.x), min.y.min(point.y))
        });
    let screen_max = projected_corners.iter().fold(
        egui::pos2(f32::NEG_INFINITY, f32::NEG_INFINITY),
        |max, point| egui::pos2(max.x.max(point.x), max.y.max(point.y)),
    );
    let bounds = egui::Rect::from_min_max(screen_min, screen_max).expand(5.0);
    let painter = ui.painter_at(viewport);
    painter.rect_filled(
        bounds,
        egui::CornerRadius::same(2),
        egui::Color32::from_rgba_unmultiplied(80, 140, 255, 18),
    );
    painter.rect_stroke(
        bounds,
        egui::CornerRadius::same(2),
        egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(105, 165, 255)),
        egui::StrokeKind::Inside,
    );
    if app.ui_state.placing_text_layer.is_some() {
        let handle = text_world_to_screen(app, [max[0], min[1]], viewport);
        let handle_rect = egui::Rect::from_center_size(handle, egui::vec2(8.0, 8.0));
        painter.rect_filled(handle_rect, 1.0, egui::Color32::WHITE);
        painter.rect_stroke(
            handle_rect,
            egui::CornerRadius::same(1),
            egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(55, 115, 215)),
            egui::StrokeKind::Inside,
        );
        let button_pos = egui::pos2(bounds.center().x, bounds.top() - 14.0);
        egui::Area::new(egui::Id::new("text_rotate_button"))
            .order(egui::Order::Foreground)
            .fixed_pos(button_pos)
            .pivot(egui::Align2::CENTER_CENTER)
            .show(ui.ctx(), |ui| {
                let response = ui
                    .add_sized(
                        egui::vec2(24.0, 24.0),
                        egui::Button::new(egui::RichText::new("⟳").size(16.0))
                            .sense(egui::Sense::click_and_drag()),
                    )
                    .on_hover_cursor(egui::CursorIcon::Grab)
                    .on_hover_text("Drag to rotate text");
                if response.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                }
                if response.drag_started_by(egui::PointerButton::Primary) {
                    if let Some(pointer) = response.interact_pointer_pos() {
                        if let Some(layer) = app.project.layers.get(index) {
                            if let Some((min, max)) = layer.geometry.bounds() {
                                let center = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
                                let cursor = screen_to_text_world(app, pointer, viewport);
                                app.ui_state.text_rotate = Some(crate::app::TextRotateState {
                                    layer_index: index,
                                    original_geometry: layer.geometry.clone(),
                                    center,
                                    initial_angle: (cursor.y - center[1])
                                        .atan2(cursor.x - center[0]),
                                    initial_rotation_degrees: layer
                                        .text_data
                                        .as_ref()
                                        .map_or(0.0, |data| data.rotation_degrees),
                                });
                            }
                        }
                    }
                }
                if let (Some(rotate), Some(pointer)) = (
                    app.ui_state.text_rotate.clone(),
                    response.interact_pointer_pos(),
                ) {
                    if response.dragged_by(egui::PointerButton::Primary) {
                        let cursor = screen_to_text_world(app, pointer, viewport);
                        rotate_text_layer(app, &rotate, [cursor.x, cursor.y]);
                    }
                }
                if response.drag_stopped_by(egui::PointerButton::Primary)
                    && app.ui_state.text_rotate.is_some()
                {
                    app.ui_state.text_rotate = None;
                    app.ui_state.project_dirty = true;
                }
            });
    }
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
