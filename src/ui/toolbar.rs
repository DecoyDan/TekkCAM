use crate::app::TekkCamApp;
use eframe::egui;

const DANGER: egui::Color32 = egui::Color32::from_rgb(220, 70, 70);
const ICON_LAYOUT_SIDEBAR_LEFT: char = '\u{EBF3}';
const ICON_LAYOUT_SIDEBAR_LEFT_OFF: char = '\u{EC02}';
const ICON_LAYOUT_SIDEBAR_RIGHT: char = '\u{EBF4}';
const ICON_LAYOUT_SIDEBAR_RIGHT_OFF: char = '\u{EC00}';
const ICON_PLUS: char = '\u{F067}';
const ICON_CUBE_OUTLINE: char = '\u{F01A7}';
const ICON_CHECK: char = '\u{F00C}';

pub fn show(ctx: &egui::Context, app: &mut TekkCamApp) {
    let accent = if app.ui_state.theme == crate::app::AppTheme::Dark {
        crate::ui::palette::SWISS_PLUM
    } else {
        crate::ui::palette::LIGHT_ACCENT_BLUE
    };
    let success = if app.ui_state.theme == crate::app::AppTheme::Dark {
        crate::ui::palette::FISHY_HOUSE
    } else {
        egui::Color32::from_rgb(60, 200, 100)
    };
    let panel_bg = if app.ui_state.theme == crate::app::AppTheme::Light {
        egui::Color32::from_rgb(228, 229, 241)
    } else {
        crate::ui::palette::MYSTERIOUS_DEPTHS
    };
    let toolbar_border = if app.ui_state.theme == crate::app::AppTheme::Light {
        egui::Color32::from_rgb(196, 198, 210)
    } else {
        egui::Color32::from_rgb(63, 64, 72)
    };
    let menu_bar_response = egui::TopBottomPanel::top("menu_bar")
        .exact_height(42.0)
        .show_separator_line(false)
        .frame(
            egui::Frame::new()
                .fill(panel_bg)
                .inner_margin(egui::Margin::symmetric(10, 0)),
        )
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.horizontal_centered(|ui| {
                ui.label(
                    egui::RichText::new("⚙ TekkCAM")
                        .size(16.0)
                        .color(accent)
                        .strong(),
                );
                ui.menu_button("File", |ui| {
                    if ui.button("📁  Open Project…").clicked() {
                        app.request_project_open_dialog(ctx);
                        ui.close_menu();
                    }
                    if ui.button("📂  Open File…").clicked() {
                        app.request_open_dialog(ctx);
                        ui.close_menu();
                    }
                    if ui.button("💾  Save Project").clicked() {
                        if app.project_path.is_some() {
                            app.save_project();
                        } else {
                            app.request_project_save_dialog(ctx);
                        }
                        ui.close_menu();
                    }
                    if ui.button("💾  Save Project As…").clicked() {
                        app.request_project_save_dialog(ctx);
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("📤  Export G-code…").clicked() {
                        crate::ui::console::export_gcode(ctx, app);
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("✖  Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Edit", |ui| {
                    if ui.button("↩  Undo").clicked() {
                        app.undo_project();
                        ui.close_menu();
                    }
                    if ui.button("↪  Redo").clicked() {
                        app.redo_project();
                        ui.close_menu();
                    }
                });
                ui.menu_button("View", |ui| {
                    if ui.button("🔲  Fit to Window").clicked() {
                        app.ui_state.request_fit = true;
                        ui.close_menu();
                    }
                    if ui.button("⊞  Toggle Grid").clicked() {
                        app.ui_state.show_grid = !app.ui_state.show_grid;
                        ui.close_menu();
                    }
                    ui.separator();
                    ui.add_enabled_ui(app.ui_state.show_3d_view, |ui| {
                        ui.checkbox(
                            &mut app.ui_state.show_stock_removal,
                            "Stock removal preview (sampled)",
                        );
                    });
                });
                ui.menu_button("Machine", |ui| {
                    if ui.button("🔌  Connect…").clicked() {
                        app.refresh_machine_ports();
                        app.ui_state.show_machine_dialog = true;
                        ui.close_menu();
                    }
                    if app.ui_state.machine_connected && ui.button("🎛  Controls / DRO").clicked()
                    {
                        app.ui_state.show_machine_controls = true;
                        ui.close_menu();
                    }
                    if ui.button("▶  Run Job").clicked() {
                        app.start_machine_job();
                        ui.close_menu();
                    }
                    if ui.button("⏹  Stop").clicked() {
                        if let Some(machine) = &app.machine {
                            machine.stop();
                        }
                        app.ui_state.job_progress = None;
                        ui.close_menu();
                    }
                    if ui.button("🏠  Home").clicked() {
                        if let Some(machine) = &app.machine {
                            machine.send_line("$H".into());
                        }
                        ui.close_menu();
                    }
                });
                if ui.button("Settings").clicked() {
                    app.ui_state.show_settings = true;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (badge_color, badge_icon, badge_text) = if app.ui_state.machine_connected {
                        (success, '\u{F00C}', "Connected")
                    } else {
                        (DANGER, '\u{F111}', "Disconnected")
                    };
                    let mut badge_label = egui::text::LayoutJob::default();
                    badge_label.append(
                        &badge_icon.to_string(),
                        0.0,
                        egui::TextFormat {
                            font_id: egui::FontId::new(
                                12.0,
                                egui::FontFamily::Name("download_icon".into()),
                            ),
                            color: badge_color,
                            valign: egui::Align::Center,
                            ..Default::default()
                        },
                    );
                    badge_label.append(
                        &format!("   {badge_text}"),
                        0.0,
                        egui::TextFormat {
                            font_id: egui::FontId::proportional(12.0),
                            color: badge_color,
                            valign: egui::Align::Center,
                            ..Default::default()
                        },
                    );
                    if ui.add(egui::Button::new(badge_label).frame(true)).clicked() {
                        if app.ui_state.machine_connected {
                            app.ui_state.show_machine_controls = true;
                        } else {
                            app.refresh_machine_ports();
                            app.ui_state.show_machine_dialog = true;
                        }
                    }

                    if app.has_active_project() {
                        // Add buttons directly into the right-to-left layout (no
                        // nested horizontal wrapper) so there is no phantom gap.
                        // RTL reverses order: add left-sidebar first so it
                        // appears on the right, then right-sidebar so it
                        // appears on the left — matching the icon glyphs.
                        let prev_spacing = ui.spacing().item_spacing.x;
                        ui.spacing_mut().item_spacing.x = 2.0;

                        if ui
                            .add_sized(
                                [26.0, 28.0],
                                egui::Button::new(
                                    egui::RichText::new(
                                        if app.ui_state.show_right_sidebar {
                                            ICON_LAYOUT_SIDEBAR_RIGHT
                                        } else {
                                            ICON_LAYOUT_SIDEBAR_RIGHT_OFF
                                        }
                                        .to_string(),
                                    )
                                    .font(egui::FontId::new(
                                        18.0,
                                        egui::FontFamily::Name("sidebar_icons".into()),
                                    )),
                                )
                                .fill(egui::Color32::TRANSPARENT)
                                .stroke(egui::Stroke::NONE),
                            )
                            .on_hover_text("Toggle right sidebar")
                            .clicked()
                        {
                            app.ui_state.show_right_sidebar = !app.ui_state.show_right_sidebar;
                        }

                        if ui
                            .add_sized(
                                [26.0, 28.0],
                                egui::Button::new(
                                    egui::RichText::new(
                                        if app.ui_state.show_left_sidebar {
                                            ICON_LAYOUT_SIDEBAR_LEFT
                                        } else {
                                            ICON_LAYOUT_SIDEBAR_LEFT_OFF
                                        }
                                        .to_string(),
                                    )
                                    .font(egui::FontId::new(
                                        18.0,
                                        egui::FontFamily::Name("sidebar_icons".into()),
                                    )),
                                )
                                .fill(egui::Color32::TRANSPARENT)
                                .stroke(egui::Stroke::NONE),
                            )
                            .on_hover_text("Toggle layers and operations panel")
                            .clicked()
                        {
                            app.ui_state.show_left_sidebar = !app.ui_state.show_left_sidebar;
                        }

                        ui.spacing_mut().item_spacing.x = prev_spacing;
                    }

                    if ui.available_width() > 260.0 {
                        ui.label(
                            egui::RichText::new(&app.ui_state.status_message)
                                .size(11.0)
                                .color(egui::Color32::from_rgb(140, 140, 160)),
                        );
                    }
                });
            });
        });

    let mut tool_bar_rect = None;
    if app.has_active_project() {
        let tool_bar_response = egui::TopBottomPanel::top("tool_creation_toolbar")
            .exact_height(40.0)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(panel_bg)
                    .inner_margin(egui::Margin::symmetric(10, 0)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::horizontal()
                    .id_salt("toolbar_controls")
                    .auto_shrink([false, false])
                    .scroll_bar_visibility(
                        egui::containers::scroll_area::ScrollBarVisibility::AlwaysHidden,
                    )
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.horizontal_centered(|ui| {
                    ui.menu_button(
                        egui::RichText::new(ICON_PLUS.to_string()).font(egui::FontId::new(
                            16.0,
                            egui::FontFamily::Name("sidebar_icons".into()),
                        )),
                        |ui| {
                            let operations = [
                                (
                                    "〰  Isolation Routing",
                                    crate::cam::operations::Operation::isolation(),
                                ),
                                (
                                    "⬡  Pocket Milling",
                                    crate::cam::operations::Operation::pocket(),
                                ),
                                ("⊙  Drilling", crate::cam::operations::Operation::drill()),
                                (
                                    "⊙  Helical Bore (mill holes)",
                                    crate::cam::operations::Operation::helical_bore(),
                                ),
                                (
                                    "▭  Contour Cut",
                                    crate::cam::operations::Operation::contour(),
                                ),
                                (
                                    "⌑  STEP Face Profile",
                                    crate::cam::operations::Operation::model_profile_2d(),
                                ),
                                (
                                    "▦  3D Stock Roughing",
                                    crate::cam::operations::Operation::roughing_3d(),
                                ),
                                (
                                    "〰  3D Raster Finish",
                                    crate::cam::operations::Operation::finish_raster_3d(),
                                ),
                                (
                                    "◎  3D Contour-Parallel Finish",
                                    crate::cam::operations::Operation::finish_contour_3d(),
                                ),
                                (
                                    "≋  3D Waterline Finish",
                                    crate::cam::operations::Operation::finish_waterline_3d(),
                                ),
                            ];
                            for (label, operation) in operations {
                                if ui.button(label).clicked() {
                                    app.add_operation(operation);
                                    ui.close_menu();
                                }
                            }
                            let plugin_entries = app.operation_plugins();
                            if !plugin_entries.is_empty() {
                                ui.separator();
                                ui.label("Custom operations");
                                for (id, name) in plugin_entries {
                                    if ui.button(format!("⚙  {name}")).clicked() {
                                        app.add_operation(
                                            crate::cam::operations::Operation::custom(
                                                id,
                                                crate::cam::tool::Tool::default(),
                                            ),
                                        );
                                        ui.close_menu();
                                    }
                                }
                            }
                        },
                    )
                    .response
                    .on_hover_text("Add CAM operation");
                    let selected_operation = app
                        .ui_state
                        .selected_operation
                        .filter(|&index| index < app.project.operations.len());
                    if ui
                        .add_enabled(
                            selected_operation.is_some() && !app.generation_pending,
                            egui::Button::new("▶ Generate Toolpath"),
                        )
                        .clicked()
                    {
                        if let Some(index) = selected_operation {
                            app.generate_operation(index);
                        }
                    }
                    if ui.button("▶ Generate All").clicked() {
                        for i in 0..app.project.operations.len() {
                            app.generate_operation(i);
                        }
                    }
                    if app.plan_pending {
                        if ui
                            .button("✖ Cancel plan")
                            .on_hover_text("Stop the running auto plan; nothing is changed")
                            .clicked()
                        {
                            app.cancel_auto_plan();
                        }
                    } else {
                        let can_auto_plan = app.project.models.iter().any(|model| model.visible)
                            && !app.generation_pending;
                        if ui
                            .add_enabled(can_auto_plan, egui::Button::new("⚙ Auto Plan"))
                            .on_hover_text(
                                "Pick and generate every operation this model needs (roughing, helical bores, finishing)",
                            )
                            .clicked()
                        {
                            app.ui_state.show_auto_plan_dialog = true;
                        }
                    }
                    let can_select_model_faces = app.ui_state.show_3d_view
                        && app.project.models.iter().any(|model| model.visible)
                        && app
                            .ui_state
                            .selected_operation
                            .and_then(|index| app.project.operations.get(index))
                            .is_some_and(|operation| {
                                matches!(
                                    &operation.kind,
                                    crate::cam::operations::OperationKind::ModelProfile2D
                                )
                            });
                    let icon = if app.ui_state.face_selection_mode {
                        ICON_CHECK
                    } else {
                        ICON_CUBE_OUTLINE
                    };
                    let mut face_selection_label = egui::text::LayoutJob::default();
                    face_selection_label.append(
                        &icon.to_string(),
                        0.0,
                        egui::TextFormat {
                            font_id: egui::FontId::new(
                                14.0,
                                egui::FontFamily::Name(
                                    if app.ui_state.face_selection_mode {
                                        "download_icon".into()
                                    } else {
                                        "sidebar_icons".into()
                                    },
                                ),
                            ),
                            ..Default::default()
                        },
                    );
                    face_selection_label.append(
                        " Select Faces",
                        0.0,
                        egui::TextFormat {
                            font_id: egui::FontId::proportional(14.0),
                            ..Default::default()
                        },
                    );
                    if ui
                        .add_enabled(
                            can_select_model_faces,
                            egui::Button::new(face_selection_label),
                        )
                        .on_hover_text("Click model faces to add or remove them from the selected profile operation")
                        .clicked()
                    {
                        app.ui_state.face_selection_mode = !app.ui_state.face_selection_mode;
                        if app.ui_state.face_selection_mode {
                            if let Some(operation) = app
                                .ui_state
                                .selected_operation
                                .and_then(|index| app.project.operations.get_mut(index))
                            {
                                if !operation.manual_face_selection {
                                    operation.manual_face_selection = true;
                                    operation.source_face_ids.clear();
                                    app.ui_state.project_dirty = true;
                                }
                            }
                            app.ui_state.status_message =
                                "Click model faces to select them; click again to deselect".into();
                        } else {
                            app.ui_state.status_message = "Face selection finished".into();
                        }
                    }
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(30.0, 26.0), egui::Sense::click());
                    if response.hovered() || response.is_pointer_button_down_on() {
                        ui.painter()
                            .rect_filled(rect, 4.0, ui.visuals().widgets.hovered.bg_fill);
                    }
                    let center = rect.center();
                    let stroke = egui::Stroke::new(1.8_f32, ui.visuals().text_color());
                    ui.painter().rect_stroke(
                        egui::Rect::from_min_max(
                            egui::pos2(center.x - 3.5, center.y - 7.0),
                            egui::pos2(center.x + 3.5, center.y + 1.0),
                        ),
                        1.0,
                        stroke,
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().line_segment(
                        [
                            egui::pos2(center.x - 3.5, center.y + 1.0),
                            egui::pos2(center.x, center.y + 7.0),
                        ],
                        stroke,
                    );
                    ui.painter().line_segment(
                        [
                            egui::pos2(center.x + 3.5, center.y + 1.0),
                            egui::pos2(center.x, center.y + 7.0),
                        ],
                        stroke,
                    );
                    let badge = egui::pos2(rect.right() - 6.0, rect.top() + 6.0);
                    ui.painter().circle_filled(badge, 5.0, panel_bg);
                    ui.painter()
                        .circle_stroke(badge, 4.5, egui::Stroke::new(1.2_f32, accent));
                    ui.painter().line_segment(
                        [
                            egui::pos2(badge.x - 2.0, badge.y),
                            egui::pos2(badge.x + 2.0, badge.y),
                        ],
                        egui::Stroke::new(1.2_f32, accent),
                    );
                    ui.painter().line_segment(
                        [
                            egui::pos2(badge.x, badge.y - 2.0),
                            egui::pos2(badge.x, badge.y + 2.0),
                        ],
                        egui::Stroke::new(1.2_f32, accent),
                    );
                    if response.clicked() {
                        app.ui_state.tool_editor = Some(crate::cam::tool::Tool::default());
                        app.ui_state.tool_editor_index = None;
                        app.ui_state.tool_editor_error = None;
                    }
                    response.on_hover_text("Create new tool");
                    if ui.button("Tools").clicked() {
                        app.ui_state.show_tool_database = true;
                    }
                    if ui.button("Stock").clicked() {
                        app.ui_state.show_stock_dialog = true;
                    }
                    if ui.button("Work").clicked() {
                        app.ui_state.show_work_dialog = true;
                    }
                    if ui.button("Probe").clicked() {
                        app.ui_state.show_probing_dialog = true;
                    }
                    ui.separator();
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("▶ Simulate").color(egui::Color32::WHITE),
                            )
                            .fill(egui::Color32::from_rgb(40, 140, 60)),
                        )
                        .clicked()
                    {
                        app.start_simulation();
                        ctx.request_repaint();
                    }
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("⏹ Stop").color(egui::Color32::WHITE),
                            )
                            .fill(DANGER),
                        )
                        .clicked()
                    {
                        app.ui_state.simulator.stop();
                        app.ui_state.status_message = "Simulation stopped".into();
                    }
                        });
                    });
            });
        tool_bar_rect = Some(tool_bar_response.response.rect);
    }

    // Use Background order so border lines don't paint over menus/dropdowns.
    let border_painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("toolbar_bottom_borders"),
    ));
    let border = egui::Stroke::new(1.0_f32, toolbar_border);

    let menu_rect = menu_bar_response.response.rect;

    if let Some(toolbar_rect) = tool_bar_rect {
        // Single divider between the two bars: draw it at the shared edge
        // (bottom of menu bar == top of toolbar). Using floor() keeps it crisp.
        let y = (menu_rect.bottom()).floor();
        // Both panels span the full window width — use the menu bar's x_range.
        border_painter.hline(menu_rect.x_range(), y - 0.5, border);

        // Bottom edge of the toolbar
        let y_bottom = toolbar_rect.bottom().floor();
        border_painter.hline(toolbar_rect.x_range(), y_bottom - 0.5, border);
    } else {
        // No toolbar — just draw the bottom of the menu bar
        let y = menu_rect.bottom().floor();
        border_painter.hline(menu_rect.x_range(), y - 0.5, border);
    }
}
