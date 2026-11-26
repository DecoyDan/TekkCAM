use crate::app::TekkCamApp;
use crate::gcode::generator::Dialect;
use eframe::egui;
use egui::text::{LayoutJob, TextFormat};

const DANGER: egui::Color32 = egui::Color32::from_rgb(220, 70, 70);

pub fn show_in_sidebar(ui: &mut egui::Ui, ctx: &egui::Context, app: &mut TekkCamApp) {
    let accent = if app.ui_state.theme == crate::app::AppTheme::Dark {
        crate::ui::palette::SWISS_PLUM
    } else {
        crate::ui::palette::LIGHT_ACCENT_BLUE
    };
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(if app.ui_state.show_gcode_editor {
                "G-CODE EDITOR"
            } else {
                "CONSOLE / G-CODE"
            })
            .size(10.0)
            .color(egui::Color32::from_rgb(140, 160, 220))
            .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let dark = app.ui_state.theme == crate::app::AppTheme::Dark;
            let action_fill = if dark {
                crate::ui::palette::CHARCOAL_SURFACE
            } else {
                egui::Color32::from_rgb(210, 211, 219)
            };
            let action_text = if dark {
                egui::Color32::WHITE
            } else {
                egui::Color32::from_rgb(72, 75, 106)
            };
            let action_button = |label: &str| {
                egui::Button::new(egui::RichText::new(label).size(12.0).color(action_text))
                    .fill(action_fill)
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(egui::CornerRadius::same(5))
            };
            let clear_button = action_button("Clear");
            if ui.add_sized([58.0, 28.0], clear_button).clicked() {
                app.ui_state.console_log.clear();
            }
            let editor_label = if app.ui_state.show_gcode_editor {
                "Console"
            } else {
                "Editor"
            };
            if ui
                .add_sized([72.0, 28.0], action_button(editor_label))
                .clicked()
            {
                app.ui_state.show_gcode_editor = !app.ui_state.show_gcode_editor;
            }
        });
    });
    ui.horizontal(|ui| {
        let dark = app.ui_state.theme == crate::app::AppTheme::Dark;
        let active_fill = crate::ui::palette::SWISS_PLUM;
        egui::Frame::new()
            .fill(if dark {
                crate::ui::palette::MYSTERIOUS_DEPTHS
            } else {
                egui::Color32::from_rgb(215, 217, 229)
            })
            .corner_radius(egui::CornerRadius::same(7))
            .inner_margin(egui::Margin::same(3))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.horizontal(|ui| {
                    for (dialect, label) in
                        [(Dialect::Grbl, "GRBL"), (Dialect::LinuxCnc, "LinuxCNC")]
                    {
                        let selected = app.project.post_dialect == dialect;
                        let button = egui::Button::new(
                            egui::RichText::new(label)
                                .size(12.0)
                                .strong()
                                .color(if selected {
                                    egui::Color32::WHITE
                                } else if dark {
                                    egui::Color32::from_rgb(165, 168, 185)
                                } else {
                                    egui::Color32::from_rgb(72, 75, 106)
                                }),
                        )
                        .fill(if selected {
                            active_fill
                        } else {
                            egui::Color32::TRANSPARENT
                        })
                        .stroke(egui::Stroke::NONE)
                        .corner_radius(egui::CornerRadius::same(5));
                        if ui.add_sized([82.0, 28.0], button).clicked() && !selected {
                            app.project.post_dialect = dialect;
                            app.ui_state.project_dirty = true;
                        }
                    }
                });
            });
        if ui.button("Generate G-code").clicked() {
            app.generate_gcode();
        }
        if ui.button("Export").clicked() {
            export_gcode(ctx, app);
        }
    });
    ui.separator();
    let body_height = (ui.available_height() - 54.0).max(60.0);
    if app.ui_state.show_gcode_editor {
        let dark = app.ui_state.theme == crate::app::AppTheme::Dark;
        let mut layouter = |ui: &egui::Ui, text: &str, width: f32| {
            ui.fonts(|fonts| fonts.layout_job(highlight_gcode(text, width, dark)))
        };
        egui::ScrollArea::vertical()
            .id_salt("gcode_editor_scroll")
            .drag_to_scroll(false)
            .auto_shrink([false, false])
            .max_height(body_height)
            .show(ui, |ui| {
                apply_dark_input_border(ui, dark);
                ui.add_sized(
                    [ui.available_width(), body_height],
                    egui::TextEdit::multiline(&mut app.ui_state.gcode_text)
                        .font(egui::FontId::monospace(12.0))
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .layouter(&mut layouter),
                );
            });
    } else {
        egui::ScrollArea::vertical()
            .id_salt("console_log")
            .drag_to_scroll(false)
            .auto_shrink([false, false])
            .max_height(body_height)
            .stick_to_bottom(true)
            .show_rows(
                ui,
                ui.text_style_height(&egui::TextStyle::Monospace),
                app.ui_state.console_log.len(),
                |ui, visible_rows| {
                    for row in visible_rows {
                        let line = &app.ui_state.console_log[row];
                        let color = if line.starts_with("✖") || line.starts_with("Error") {
                            DANGER
                        } else if line.starts_with("✔") || line.contains("ready") {
                            if app.ui_state.theme == crate::app::AppTheme::Dark {
                                crate::ui::palette::FISHY_HOUSE
                            } else {
                                egui::Color32::from_rgb(60, 200, 100)
                            }
                        } else if app.ui_state.theme == crate::app::AppTheme::Light {
                            egui::Color32::from_rgb(72, 75, 106)
                        } else {
                            egui::Color32::from_rgb(180, 200, 180)
                        };
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(line)
                                    .monospace()
                                    .size(12.0)
                                    .color(color),
                            )
                            .truncate(),
                        );
                    }
                },
            );
    }
    ui.separator();
    ui.horizontal(|ui| {
        apply_dark_input_border(ui, app.ui_state.theme == crate::app::AppTheme::Dark);
        ui.label(egui::RichText::new("›").color(accent).size(14.0));
        let response = ui.add_sized(
            [(ui.available_width() - 65.0).max(50.0), 22.0],
            egui::TextEdit::singleline(&mut app.ui_state.console_input)
                .hint_text("Enter G-code command…")
                .font(egui::FontId::monospace(12.0)),
        );
        if ui.button("Send").clicked()
            || (response.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter)))
        {
            if !app.ui_state.console_input.trim().is_empty() {
                let command = app.ui_state.console_input.trim().to_owned();
                if let Some(machine) = &app.machine {
                    machine.send_line(command.clone());
                } else {
                    app.ui_state
                        .console_log
                        .push("✖ Connect to a machine before sending commands".into());
                }
                app.ui_state.console_log.push(format!("› {command}"));
                app.ui_state.console_input.clear();
            }
        }
    });
}

pub fn export_gcode(ctx: &egui::Context, app: &mut TekkCamApp) {
    app.request_export_dialog(ctx);
}

fn highlight_gcode(text: &str, width: f32, dark: bool) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = width;
    let mono = egui::FontId::monospace(12.0);
    let base = TextFormat {
        font_id: mono.clone(),
        color: if dark {
            egui::Color32::from_rgb(205, 210, 220)
        } else {
            egui::Color32::from_rgb(56, 59, 86)
        },
        ..Default::default()
    };
    let command = TextFormat {
        font_id: mono.clone(),
        color: if dark {
            crate::ui::palette::SWISS_PLUM
        } else {
            egui::Color32::from_rgb(38, 95, 200)
        },
        ..Default::default()
    };
    let axis = TextFormat {
        font_id: mono.clone(),
        color: if dark {
            egui::Color32::from_rgb(245, 190, 105)
        } else {
            egui::Color32::from_rgb(185, 95, 15)
        },
        ..Default::default()
    };
    let comment = TextFormat {
        font_id: mono,
        color: if dark {
            egui::Color32::from_rgb(105, 165, 115)
        } else {
            egui::Color32::from_rgb(46, 125, 62)
        },
        ..Default::default()
    };
    for line in text.split_inclusive('\n') {
        for token in line.split_inclusive(char::is_whitespace) {
            let style =
                if token.trim_start().starts_with('(') || token.trim_start().starts_with(';') {
                    &comment
                } else if matches!(token.chars().next(), Some('G' | 'M' | 'T')) {
                    &command
                } else if matches!(
                    token.chars().next(),
                    Some('X' | 'Y' | 'Z' | 'I' | 'J' | 'F' | 'S')
                ) {
                    &axis
                } else {
                    &base
                };
            job.append(token, 0.0, style.clone());
        }
    }
    job
}

pub fn simulation_position(app: &TekkCamApp) -> Option<[f64; 2]> {
    app.ui_state.simulator.position
}

fn apply_dark_input_border(ui: &mut egui::Ui, dark: bool) {
    if dark {
        let border = egui::Stroke::new(1.0_f32, crate::ui::palette::SWISS_PLUM);
        ui.visuals_mut().widgets.inactive.bg_stroke = border;
        ui.visuals_mut().selection.stroke = border;
    }
}
