use crate::app::TekkCamApp;
use crate::renderer::pipeline::Scene;
use crate::renderer::ViewportCallback;
use eframe::egui;
use std::sync::Arc;

pub fn show(ctx: &egui::Context, app: &mut TekkCamApp) {
    let viewport_bg = if app.ui_state.theme == crate::app::AppTheme::Light {
        egui::Color32::from_rgb(250, 250, 250)
    } else {
        crate::ui::palette::MYSTERIOUS_DEPTHS
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

            if app.project.models.is_empty() && app.project.layers.is_empty() {
                show_empty_workspace(ui, app, rect);
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
                    grid_origin_mm: [
                        app.project.setup.stock_origin_mm[0] as f32,
                        app.project.setup.stock_origin_mm[1] as f32,
                    ],
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
                // The tool that is running this move (each operation has its own).
                let tool = crate::gcode::simulator::tool_at(
                    &app.ui_state.simulation_ranges,
                    app.ui_state.simulator.move_index,
                )
                .cloned()
                .or_else(|| {
                    app.ui_state
                        .selected_operation
                        .and_then(|index| app.project.operations.get(index))
                        .map(|operation| operation.tool.clone())
                })
                .unwrap_or_else(|| app.project.default_tool.clone());
                paint_simulated_tool(
                    &painter,
                    app,
                    rect,
                    &tool,
                    [world_xy[0] as f32, world_xy[1] as f32, world_z as f32],
                    screen,
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
    let dark = app.ui_state.theme == crate::app::AppTheme::Dark;
    let move_count = app.ui_state.simulation_moves.len();
    let completed =
        app.ui_state.simulator.move_index as f32 + app.ui_state.simulator.fraction as f32;
    let mut progress = (completed / move_count as f32).clamp(0.0, 1.0);
    egui::Area::new(egui::Id::new("simulation_playback_controls"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -50.0))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(if dark {
                    egui::Color32::from_rgba_premultiplied(18, 22, 30, 238)
                } else {
                    egui::Color32::from_rgb(242, 243, 249)
                })
                .stroke(egui::Stroke::new(
                    1.0_f32,
                    if dark {
                        egui::Color32::from_rgb(65, 83, 108)
                    } else {
                        egui::Color32::from_rgb(211, 213, 226)
                    },
                ))
                .corner_radius(egui::CornerRadius::same(7))
                .inner_margin(egui::Margin::symmetric(10, 7))
                .show(ui, |ui| {
                    ui.set_min_width(720.0);
                    ui.horizontal(|ui| {
                        let mut play_button = egui::Button::new(
                            egui::RichText::new(if app.ui_state.simulator.running {
                                "⏸"
                            } else {
                                "▶"
                            })
                            .color(egui::Color32::WHITE),
                        );
                        if !dark {
                            play_button = play_button
                                .fill(crate::ui::palette::SWISS_PLUM)
                                .stroke(egui::Stroke::NONE);
                        }
                        let play_pause = ui.add_sized([30.0, 24.0], play_button).on_hover_text(
                            if app.ui_state.simulator.running {
                                "Pause simulation"
                            } else {
                                "Play simulation"
                            },
                        );
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
                        let slider = ui
                            .scope(|ui| {
                                let default_slider_width = ui.spacing().slider_width;
                                ui.spacing_mut().slider_width = 480.0;
                                if !dark {
                                    ui.visuals_mut().selection.bg_fill =
                                        crate::ui::palette::SWISS_PLUM;
                                    ui.visuals_mut().selection.stroke.color =
                                        crate::ui::palette::SWISS_PLUM;
                                }
                                let slider = ui.add_sized(
                                    [480.0, 18.0],
                                    egui::Slider::new(&mut progress, 0.0..=1.0).show_value(false),
                                );
                                ui.spacing_mut().slider_width = default_slider_width;
                                slider
                            })
                            .inner;
                        if slider.changed() {
                            app.ui_state
                                .simulator
                                .seek(&app.ui_state.simulation_moves, progress);
                        }
                        ui.label(format!("{:.0}%", progress * 100.0));
                        ui.separator();
                        for speed in [1.0_f32, 2.5, 5.0, 10.0] {
                            let selected = app.ui_state.simulation_speed == speed;
                            let text_color = if selected {
                                egui::Color32::WHITE
                            } else if dark {
                                egui::Color32::from_rgb(165, 168, 185)
                            } else {
                                egui::Color32::from_rgb(72, 75, 106)
                            };
                            let speed_button = egui::Button::new(
                                egui::RichText::new(format!("{speed}×")).color(text_color),
                            )
                            .selected(selected)
                            .fill(if selected {
                                crate::ui::palette::SWISS_PLUM
                            } else {
                                egui::Color32::TRANSPARENT
                            })
                            .stroke(egui::Stroke::NONE)
                            .corner_radius(egui::CornerRadius::same(4));
                            if ui.add(speed_button).clicked() {
                                app.ui_state.simulation_speed = speed;
                            }
                        }
                    });
                });
        });
}

fn show_stock_render_controls(ctx: &egui::Context, app: &mut TekkCamApp, viewport: egui::Rect) {
    let dark = app.ui_state.theme == crate::app::AppTheme::Dark;
    egui::Area::new(egui::Id::new("stock_render_controls"))
        .order(egui::Order::Middle)
        .fixed_pos(egui::pos2(viewport.left() + 12.0, viewport.bottom() - 12.0))
        .pivot(egui::Align2::LEFT_BOTTOM)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(if dark {
                    egui::Color32::from_rgba_premultiplied(18, 22, 30, 225)
                } else {
                    egui::Color32::from_rgb(215, 217, 229)
                })
                .stroke(egui::Stroke::new(
                    1.0_f32,
                    if dark {
                        egui::Color32::from_rgb(65, 83, 108)
                    } else {
                        egui::Color32::from_rgb(211, 213, 226)
                    },
                ))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(7, 5))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let view_button = |label: &str, selected: bool| {
                            let text_color = if selected {
                                egui::Color32::WHITE
                            } else if dark {
                                egui::Color32::from_rgb(165, 168, 185)
                            } else {
                                egui::Color32::from_rgb(72, 75, 106)
                            };
                            egui::Button::new(
                                egui::RichText::new(label).size(12.0).color(text_color),
                            )
                            .selected(selected)
                            .fill(if selected {
                                crate::ui::palette::SWISS_PLUM
                            } else {
                                egui::Color32::TRANSPARENT
                            })
                            .stroke(egui::Stroke::NONE)
                            .corner_radius(egui::CornerRadius::same(4))
                        };
                        if ui
                            .add(view_button("2D", !app.ui_state.show_3d_view))
                            .clicked()
                        {
                            app.ui_state.show_3d_view = false;
                            app.ui_state.render_stock = false;
                            app.ui_state.request_fit = true;
                        }
                        if ui
                            .add(view_button(
                                "3D",
                                app.ui_state.show_3d_view && !app.ui_state.render_stock,
                            ))
                            .clicked()
                        {
                            app.ui_state.show_3d_view = true;
                            app.ui_state.render_stock = false;
                            app.ui_state.show_stock_removal = false;
                            app.ui_state.model_orbit = [0.0; 2];
                            app.ui_state.request_fit = true;
                        }
                        if ui
                            .add(view_button("Render", app.ui_state.render_stock))
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
    let dark = app.ui_state.theme == crate::app::AppTheme::Dark;
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
        egui::vec2(300.0, 44.0),
    );
    let painter = ui.painter_at(viewport);
    painter.rect_filled(
        rect,
        egui::CornerRadius::same(6),
        if dark {
            egui::Color32::from_rgba_premultiplied(
                (18.0 * animation) as u8,
                (22.0 * animation) as u8,
                (30.0 * animation) as u8,
                (230.0 * animation) as u8,
            )
        } else {
            egui::Color32::from_rgba_unmultiplied(242, 243, 249, (248.0 * animation) as u8)
        },
    );
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(6),
        egui::Stroke::new(
            1.0_f32,
            if dark {
                egui::Color32::from_rgba_unmultiplied(65, 83, 108, (255.0 * animation) as u8)
            } else {
                egui::Color32::from_rgba_unmultiplied(211, 213, 226, (255.0 * animation) as u8)
            },
        ),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.left_top() + egui::vec2(8.0, 5.0),
        egui::Align2::LEFT_TOP,
        "SIMULATION DRO",
        egui::FontId::proportional(9.0),
        if dark {
            egui::Color32::from_rgba_unmultiplied(115, 205, 145, (255.0 * animation) as u8)
        } else {
            egui::Color32::from_rgba_unmultiplied(89, 80, 184, (255.0 * animation) as u8)
        },
    );
    for (index, (axis, value)) in values.iter().enumerate() {
        let x = rect.left() + 8.0 + index as f32 * 96.0;
        let y = rect.top() + 24.0;
        painter.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_CENTER,
            format!("{axis} {value:>8.3}{}", units.label()),
            egui::FontId::monospace(10.5),
            if dark {
                egui::Color32::from_rgba_unmultiplied(225, 232, 242, (255.0 * animation) as u8)
            } else {
                egui::Color32::from_rgba_unmultiplied(45, 47, 64, (255.0 * animation) as u8)
            },
        );
    }
}

fn show_view_cube(ui: &mut egui::Ui, app: &mut TekkCamApp, viewport: egui::Rect) {
    let side = 38.0;
    let scale = 29.0;
    let center = egui::pos2(viewport.right() - 78.0, viewport.top() + 82.0);
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
            egui::Color32::from_rgb(62, 62, 68),
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
            egui::Color32::from_rgb(45, 45, 50),
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
            egui::Color32::from_rgb(78, 78, 84),
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
            egui::Color32::from_rgb(57, 57, 63),
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
            egui::Color32::from_rgb(86, 86, 92),
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
            egui::Color32::from_rgb(68, 68, 74),
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
    let cube_rect = egui::Rect::from_center_size(center, egui::vec2(side * 2.8, side * 2.8));
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
            egui::Stroke::new(1.4_f32, egui::Color32::from_rgb(35, 35, 40)),
        ));
        painter.text(
            face_center,
            egui::Align2::CENTER_CENTER,
            *label,
            egui::FontId::proportional(11.0),
            egui::Color32::from_rgb(228, 228, 232),
        );
    }
    let axis_origin = [-1.0, -1.0, -1.0];
    let axes = [
        ("X", [1.0, 0.0, 0.0], egui::Color32::from_rgb(245, 62, 72)),
        ("Y", [0.0, 1.0, 0.0], egui::Color32::from_rgb(90, 220, 74)),
        ("Z", [0.0, 0.0, 1.0], egui::Color32::from_rgb(52, 145, 255)),
    ];
    for (label, direction, color) in axes {
        let tip = project([
            axis_origin[0] + direction[0] * 3.0,
            axis_origin[1] + direction[1] * 3.0,
            axis_origin[2] + direction[2] * 3.0,
        ]);
        let origin = project(axis_origin);
        let vector = tip - origin;
        let length = vector.length();
        if length <= f32::EPSILON {
            continue;
        }
        let unit = vector / length;
        let arrow_base = tip - unit * 7.0;
        let perpendicular = egui::vec2(-unit.y, unit.x) * 3.5;
        painter.line_segment([origin, tip], egui::Stroke::new(2.4_f32, color));
        painter.add(egui::Shape::convex_polygon(
            vec![tip, arrow_base + perpendicular, arrow_base - perpendicular],
            color,
            egui::Stroke::NONE,
        ));
        painter.text(
            tip + unit * 9.0,
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(12.0),
            color,
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
        crate::ui::palette::CHARCOAL_SURFACE
    } else {
        egui::Color32::from_rgb(242, 243, 249)
    };
    let border = if dark {
        crate::ui::palette::SURF_THE_WEB
    } else {
        egui::Color32::from_rgb(211, 213, 226)
    };
    let muted = if dark {
        egui::Color32::from_rgb(166, 165, 205)
    } else {
        egui::Color32::from_rgb(103, 107, 128)
    };
    let heading = if dark {
        egui::Color32::from_rgb(243, 241, 255)
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

fn show_empty_workspace(ui: &mut egui::Ui, app: &mut TekkCamApp, rect: egui::Rect) {
    let dark = app.ui_state.theme == crate::app::AppTheme::Dark;
    let card = if dark {
        crate::ui::palette::CHARCOAL_SURFACE
    } else {
        egui::Color32::from_rgb(242, 243, 249)
    };
    let border = if dark {
        crate::ui::palette::SURF_THE_WEB
    } else {
        egui::Color32::from_rgb(211, 213, 226)
    };
    let heading = if dark {
        egui::Color32::from_rgb(243, 241, 255)
    } else {
        egui::Color32::from_rgb(37, 39, 53)
    };
    let muted = if dark {
        egui::Color32::from_rgb(166, 165, 205)
    } else {
        egui::Color32::from_rgb(103, 107, 128)
    };
    let width = rect.width().min(620.0).max(320.0);
    let height = 316.0_f32.min(rect.height().max(270.0));
    let card_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(width, height));

    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(card_rect), |ui| {
        egui::Frame::new()
            .fill(card)
            .stroke(egui::Stroke::new(1.0_f32, border))
            .corner_radius(egui::CornerRadius::same(16))
            .inner_margin(egui::Margin::same(28))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("Bring in a design to get started")
                            .size(25.0)
                            .strong()
                            .color(heading),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(
                            "Import a drawing or 3D model. Then add a toolpath, review it, and export your G-code.",
                        )
                        .size(14.0)
                        .color(muted),
                    );
                    ui.add_space(18.0);
                    if import_design_button(ui).clicked() {
                        app.request_open_dialog(ui.ctx());
                    }
                    ui.add_space(20.0);
                    ui.separator();
                    ui.add_space(12.0);
                    for (number, title, detail) in [
                        ("1", "Import", "DXF, SVG, Gerber, drill, or STEP"),
                        ("2", "Create a toolpath", "Choose an operation and cutting tool"),
                        ("3", "Review and export", "Simulate the path, then export G-code"),
                    ] {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(number)
                                    .size(13.0)
                                    .strong()
                                    .color(if dark {
                                        crate::ui::palette::FISHY_HOUSE
                                    } else {
                                        crate::ui::palette::LIGHT_ACCENT_BLUE
                                    }),
                            );
                            ui.label(egui::RichText::new(title).strong().color(heading));
                            ui.label(egui::RichText::new(detail).color(muted));
                        });
                        ui.add_space(5.0);
                    }
                });
            });
    });
}

fn import_design_button(ui: &mut egui::Ui) -> egui::Response {
    let button_width = ui.available_width().min(160.0);
    let response = ui
        .add_sized(
            [button_width, 44.0],
            egui::Button::new("")
                .fill(crate::ui::palette::SWISS_PLUM)
                .corner_radius(egui::CornerRadius::same(9)),
        )
        .on_hover_text("Choose a design or 3D model to import");
    let center = response.rect.center();
    ui.painter().text(
        center + egui::vec2(-25.0, 0.0),
        egui::Align2::CENTER_CENTER,
        '\u{F019}',
        egui::FontId::new(15.0, egui::FontFamily::Name("download_icon".into())),
        egui::Color32::WHITE,
    );
    ui.painter().text(
        center + egui::vec2(12.0, 0.0),
        egui::Align2::CENTER_CENTER,
        "Import",
        egui::FontId::new(15.0, egui::FontFamily::Proportional),
        egui::Color32::WHITE,
    );
    response
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
                        .on_hover_text("Fit stock to model")
                        .clicked()
                    {
                        app.ui_state.selected_model = Some(model_index);
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
/// Material Design "rotate-right" glyph from the bundled Nerd Fonts icon font.
const ICON_ROTATE_RIGHT: char = '\u{F0467}';

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
        let models = app.project.models.clone();
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
                ranges: &app.ui_state.simulation_ranges,
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
    } else {
        let origin = app.project.setup.stock_origin_mm;
        let stock_size = app.project.setup.stock_size_mm;
        app.camera.fit_bounds(
            [origin[0] as f32, origin[1] as f32],
            [
                (origin[0] + stock_size[0]) as f32,
                (origin[1] + stock_size[1]) as f32,
            ],
            size,
        );
        return;
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
    // Resize handles exist only while a text layer is in its moving state
    // (rotation is driven by the rotate button drawn with the outline).
    if (primary_down || response.drag_started_by(egui::PointerButton::Primary))
        && app.ui_state.text_resize.is_none()
        && app.ui_state.text_rotate.is_none()
    {
        if let (Some(index), Some(pointer)) = (
            placing_text.filter(|&index| {
                app.project.layers.get(index).is_some_and(|layer| {
                    layer.visible && layer.kind == crate::workspace::layer::LayerKind::Text
                })
            }),
            response.interact_pointer_pos(),
        ) {
            if let Some(state) = text_resize_state_at_pointer(app, index, pointer, rect) {
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
    if let Some(resize) = &app.ui_state.text_resize {
        let cursor = resize_cursor(app, resize.initial_handle, resize.anchor, rect);
        ui.ctx().set_cursor_icon(cursor);
    } else if app.ui_state.text_rotate.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if let (Some(index), Some(pointer)) =
        (placing_text, ui.input(|input| input.pointer.hover_pos()))
    {
        if app
            .project
            .layers
            .get(index)
            .is_some_and(|layer| layer.kind == crate::workspace::layer::LayerKind::Text)
        {
            if let Some(state) = text_resize_state_at_pointer(app, index, pointer, rect) {
                let cursor = resize_cursor(app, state.initial_handle, state.anchor, rect);
                ui.ctx().set_cursor_icon(cursor);
            }
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
                    app.ui_state.status_message =
                        "Move layer with cursor and double-click to place it".into();
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
            if !layer.visible || layer.kind == crate::workspace::layer::LayerKind::Placeholder {
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
    // Pick the corner handle closest to the pointer; the opposite corner stays
    // fixed so the layer scales proportionally away from it.
    let (handle_world, anchor) = text_resize_corners(min, max)
        .into_iter()
        .map(|(handle, anchor)| (text_world_to_screen(app, handle, viewport), handle, anchor))
        .filter(|(screen, _, _)| screen.distance(pointer) <= TEXT_HANDLE_HIT_RADIUS)
        .min_by(|a, b| {
            a.0.distance(pointer)
                .partial_cmp(&b.0.distance(pointer))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(_, handle, anchor)| (handle, anchor))?;
    Some(crate::app::TextResizeState {
        layer_index: index,
        original_geometry: layer.geometry.clone(),
        anchor,
        initial_handle: handle_world,
        original_height_mm: layer.text_data.as_ref().map(|data| data.height_mm),
    })
}

const TEXT_HANDLE_HIT_RADIUS: f32 = 10.0;

/// Corner handles of a layer's bounds, each paired with the opposite corner.
fn text_resize_corners(min: [f32; 2], max: [f32; 2]) -> [([f32; 2], [f32; 2]); 4] {
    [
        ([min[0], min[1]], [max[0], max[1]]),
        ([max[0], min[1]], [min[0], max[1]]),
        ([max[0], max[1]], [min[0], min[1]]),
        ([min[0], max[1]], [max[0], min[1]]),
    ]
}

/// Diagonal resize cursor matching the on-screen direction of a corner handle.
fn resize_cursor(
    app: &TekkCamApp,
    handle: [f32; 2],
    anchor: [f32; 2],
    viewport: egui::Rect,
) -> egui::CursorIcon {
    let delta =
        text_world_to_screen(app, handle, viewport) - text_world_to_screen(app, anchor, viewport);
    if delta.x * delta.y >= 0.0 {
        egui::CursorIcon::ResizeNwSe
    } else {
        egui::CursorIcon::ResizeNeSw
    }
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
    let accent = if app.ui_state.theme == crate::app::AppTheme::Dark {
        crate::ui::palette::SWISS_PLUM
    } else {
        crate::ui::palette::LIGHT_ACCENT_BLUE
    };
    painter.rect_filled(
        bounds,
        egui::CornerRadius::same(2),
        accent.gamma_multiply(0.08),
    );
    painter.rect_stroke(
        bounds,
        egui::CornerRadius::same(2),
        egui::Stroke::new(1.5_f32, accent),
        egui::StrokeKind::Inside,
    );
    if app
        .project
        .layers
        .get(index)
        .is_some_and(|layer| layer.kind == crate::workspace::layer::LayerKind::Text)
    {
        // Resize handles on all four corners of the layer bounds.
        for (corner, _) in text_resize_corners(min, max) {
            let handle = text_world_to_screen(app, corner, viewport);
            let handle_rect = egui::Rect::from_center_size(handle, egui::vec2(9.0, 9.0));
            painter.rect_filled(handle_rect, 1.0, egui::Color32::WHITE);
            painter.rect_stroke(
                handle_rect,
                egui::CornerRadius::same(1),
                egui::Stroke::new(1.5_f32, accent),
                egui::StrokeKind::Inside,
            );
        }
        // Rotate button sits above the layer, joined to it by a short stem.
        painter.line_segment(
            [
                egui::pos2(bounds.center().x, bounds.top()),
                egui::pos2(bounds.center().x, bounds.top() - 12.0),
            ],
            egui::Stroke::new(1.5_f32, accent),
        );
        let button_pos = egui::pos2(bounds.center().x, bounds.top() - 24.0);
        egui::Area::new(egui::Id::new("text_rotate_button"))
            .order(egui::Order::Foreground)
            .fixed_pos(button_pos)
            .pivot(egui::Align2::CENTER_CENTER)
            .show(ui.ctx(), |ui| {
                let response = ui
                    .add_sized(
                        egui::vec2(24.0, 24.0),
                        egui::Button::new(
                            egui::RichText::new(ICON_ROTATE_RIGHT.to_string())
                                .font(egui::FontId::new(
                                    16.0,
                                    egui::FontFamily::Name("sidebar_icons".into()),
                                ))
                                .color(egui::Color32::WHITE),
                        )
                        .fill(accent)
                        .corner_radius(egui::CornerRadius::same(12))
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

/// Draw the cutter at its simulated position, built from the tool's real
/// dimensions: diameter, flute length, corner profile (flat / ball / bull nose),
/// shank up to the stickout, and a collet above it.
fn paint_simulated_tool(
    painter: &egui::Painter,
    app: &TekkCamApp,
    rect: egui::Rect,
    tool: &crate::cam::tool::Tool,
    tip: [f32; 3],
    tip_screen: egui::Pos2,
) {
    let zoom = app.camera.zoom;
    let pixel_diameter = tool.diameter_mm as f32 * zoom;
    let marker = |radius: f32| {
        painter.circle_filled(tip_screen, radius, egui::Color32::from_rgb(255, 75, 55));
        painter.circle_stroke(
            tip_screen,
            radius + 2.5,
            egui::Stroke::new(1.5_f32, egui::Color32::WHITE),
        );
    };
    if !app.ui_state.show_3d_view {
        // Top view: the cutter's true footprint.
        let radius = (pixel_diameter * 0.5).max(3.0);
        painter.circle_filled(
            tip_screen,
            radius,
            egui::Color32::from_rgba_unmultiplied(255, 75, 55, 90),
        );
        painter.circle_stroke(
            tip_screen,
            radius,
            egui::Stroke::new(1.5_f32, egui::Color32::WHITE),
        );
        marker(2.5);
        return;
    }
    if pixel_diameter < 5.0 {
        // Too small to see as a solid: keep a visible marker.
        marker(4.0);
        return;
    }

    let orbit = app.ui_state.model_orbit;
    let to_screen = |point: [f32; 3]| {
        let projected = crate::renderer::pipeline::project_view_point(point, orbit);
        egui::pos2(
            rect.center().x + (projected[0] - app.camera.pan.x) * zoom,
            rect.center().y - (projected[1] - app.camera.pan.y) * zoom,
        )
    };
    let radius = (tool.diameter_mm * 0.5) as f32;
    let corner = match tool.shape {
        crate::cam::tool::ToolShape::Flat => 0.0,
        crate::cam::tool::ToolShape::BallNose => radius,
        crate::cam::tool::ToolShape::BullNose => (tool.corner_radius_mm as f32).clamp(0.0, radius),
    };
    let flute = tool.flute_length_mm as f32;
    let stickout = tool.stickout_mm.max(tool.flute_length_mm) as f32;
    let collet_radius = (radius + tool.holder_clearance_mm as f32).max(radius * 1.9);
    let collet_height = (radius * 5.0).clamp(6.0, 24.0);

    // Profile from the tip upward: (radius, height above tip, part).
    // Parts: 0 = cutting flutes, 1 = shank, 2 = collet.
    let mut profile: Vec<(f32, f32, u8)> = vec![(0.0, 0.0, 0)];
    if corner > 0.0 {
        for step in 0..=6 {
            let angle = step as f32 * std::f32::consts::FRAC_PI_2 / 6.0;
            profile.push((
                radius - corner + corner * angle.sin(),
                corner - corner * angle.cos(),
                0,
            ));
        }
    } else {
        profile.push((radius, 0.0, 0));
    }
    profile.push((radius, flute.max(corner), 0));
    profile.push((radius, stickout.max(flute), 1));
    profile.push((collet_radius, stickout.max(flute), 2));
    profile.push((collet_radius, stickout.max(flute) + collet_height, 2));

    const SIDES: usize = 20;
    let view = crate::renderer::pipeline::view_direction(orbit);
    let light = {
        let v = [0.35_f32, -0.45, 0.82];
        let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        v.map(|c| c / length)
    };
    let mut quads: Vec<(f32, [egui::Pos2; 4], egui::Color32)> = Vec::new();
    for pair in profile.windows(2) {
        let ((r0, h0, _), (r1, h1, part)) = (pair[0], pair[1]);
        let (dr, dh) = (r1 - r0, h1 - h0);
        let length = dr.hypot(dh);
        if length < 1e-5 {
            continue;
        }
        // Outward normal of this profile segment as (radial, vertical).
        let (nr, nz) = (dh / length, -dr / length);
        let base = match part {
            0 => [0.80, 0.82, 0.86],
            1 => [0.62, 0.64, 0.69],
            _ => [0.30, 0.31, 0.35],
        };
        for side in 0..SIDES {
            let a0 = side as f32 * std::f32::consts::TAU / SIDES as f32;
            let a1 = (side + 1) as f32 * std::f32::consts::TAU / SIDES as f32;
            let mid = (a0 + a1) * 0.5;
            let normal = [nr * mid.cos(), nr * mid.sin(), nz];
            let facing = normal[0] * view[0] + normal[1] * view[1] + normal[2] * view[2];
            if facing <= 0.0 {
                continue;
            }
            let ring = |radius: f32, height: f32, angle: f32| {
                [
                    tip[0] + radius * angle.cos(),
                    tip[1] + radius * angle.sin(),
                    tip[2] + height,
                ]
            };
            let corners = [
                ring(r0, h0, a0),
                ring(r0, h0, a1),
                ring(r1, h1, a1),
                ring(r1, h1, a0),
            ];
            let lit = (normal[0] * light[0] + normal[1] * light[1] + normal[2] * light[2]).max(0.0);
            let shade = 0.32 + 0.68 * (0.55 * lit + 0.45 * facing);
            let color = egui::Color32::from_rgb(
                (base[0] * shade * 255.0).min(255.0) as u8,
                (base[1] * shade * 255.0).min(255.0) as u8,
                (base[2] * shade * 255.0).min(255.0) as u8,
            );
            let center = [
                (corners[0][0] + corners[2][0]) * 0.5,
                (corners[0][1] + corners[2][1]) * 0.5,
                (corners[0][2] + corners[2][2]) * 0.5,
            ];
            quads.push((
                crate::renderer::pipeline::view_depth(center, orbit),
                corners.map(to_screen),
                color,
            ));
        }
    }
    quads.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (_, points, color) in quads {
        painter.add(egui::Shape::convex_polygon(
            points.to_vec(),
            color,
            egui::Stroke::new(0.4_f32, color),
        ));
    }
    // Keep the tip easy to find.
    painter.circle_stroke(
        tip_screen,
        2.5,
        egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(255, 75, 55)),
    );
}
