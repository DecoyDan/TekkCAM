use crate::app::TekkCamApp;
use crate::cam::operations::OperationKind;
use crate::workspace::layer::{Layer, LayerKind, TextFontStyle};
use eframe::egui;

const ICON_EYE: char = '\u{F06E}';
const ICON_EYE_OFF: char = '\u{F070}';
const ICON_CHECK: char = '\u{F00C}';
const ICON_PENDING: char = '\u{F111}';
const ICON_PLUS: char = '\u{F067}';
const ACTION_BUTTON_SIZE: [f32; 2] = [32.0, 32.0];

pub fn show(ctx: &egui::Context, app: &mut TekkCamApp) {
    let panel_bg = if app.ui_state.theme == crate::app::AppTheme::Light {
        egui::Color32::from_rgb(228, 229, 241)
    } else {
        crate::ui::palette::MYSTERIOUS_DEPTHS
    };
    let dark = app.ui_state.theme == crate::app::AppTheme::Dark;
    let accent = if dark {
        crate::ui::palette::SWISS_PLUM
    } else {
        crate::ui::palette::LIGHT_ACCENT_BLUE
    };
    let selected_bg = if dark {
        crate::ui::palette::MYSTERIOUS_DEPTHS
    } else {
        egui::Color32::from_rgb(210, 211, 219)
    };
    let row_text = if dark {
        egui::Color32::from_rgb(225, 227, 236)
    } else {
        egui::Color32::from_rgb(45, 47, 64)
    };
    let header_color = if dark {
        egui::Color32::from_rgb(165, 175, 205)
    } else {
        egui::Color32::from_rgb(72, 75, 106)
    };
    egui::SidePanel::left("sidebar")
        .resizable(true)
        .default_width(240.0)
        .min_width(180.0)
        .max_width(360.0)
        .frame(
            egui::Frame::new()
                .fill(panel_bg)
                .inner_margin(egui::Margin::same(12)),
        )
        .show(ctx, |ui| {
            // ── Objects / Layers section ──────────────────────────────────
            section_header(ui, "LAYERS & TEXT", header_color);
            ui.add_space(4.0);

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
            let action_button = |text: egui::RichText| {
                egui::Button::new(text.color(action_text))
                    .fill(action_fill)
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(egui::CornerRadius::same(5))
            };

            ui.horizontal(|ui| {
                if ui
                    .add_sized(
                        ACTION_BUTTON_SIZE,
                        action_button(
                            egui::RichText::new("tT").font(egui::FontId::proportional(17.0)),
                        ),
                    )
                    .on_hover_text("Text")
                    .clicked()
                {
                    app.ui_state.editing_text_layer = None;
                    app.ui_state.text_layer_input.clear();
                    app.ui_state.text_layer_height_mm = 5.0;
                    app.ui_state.text_layer_font_style = TextFontStyle::default();
                    app.ui_state.show_text_dialog = true;
                }
                let pencil_button = ui
                    .add_sized(
                        ACTION_BUTTON_SIZE,
                        action_button(
                            egui::RichText::new("✏").font(egui::FontId::proportional(17.0)),
                        ),
                    )
                    .on_hover_text("Draw");
                if pencil_button.clicked() {
                    app.ui_state.drawing_strokes.clear();
                    app.ui_state.show_drawing_dialog = true;
                }
                if ui
                    .add_sized(
                        ACTION_BUTTON_SIZE,
                        action_button(
                            egui::RichText::new(ICON_PLUS.to_string())
                                .font(egui::FontId::new(
                                    16.0,
                                    egui::FontFamily::Name("sidebar_icons".into()),
                                )),
                        ),
                    )
                    .on_hover_text("Add STEP file")
                    .clicked()
                {
                    app.request_step_open_dialog(ctx);
                }
            });
            ui.add_space(4.0);

            // Layer list — extract display info first to avoid borrow conflicts
            let layer_count = app.project.layers.len();
            let layer_display: Vec<(bool, char, String, bool)> = (0..layer_count)
                .map(|i| {
                    let l = &app.project.layers[i];
                    let selected = app.ui_state.selected_layer == Some(i);
                    let kind_icon: &'static str = match l.kind {
                        LayerKind::Gerber => "GER",
                        LayerKind::Drill => "DRL",
                        LayerKind::Dxf => "DXF",
                        LayerKind::Svg => "SVG",
                        LayerKind::Gcode => "NC",
                        LayerKind::Placeholder => "NEW",
                        LayerKind::Text => "TXT",
                        LayerKind::Drawing => "DRW",
                    };
                    let vis_icon = if l.visible { ICON_EYE } else { ICON_EYE_OFF };
                    let label = format!("{kind_icon} {}", l.name);
                    (selected, vis_icon, label, l.visible)
                })
                .collect();

            let mut toggle_vis: Option<usize> = None;
            let mut select_layer: Option<usize> = None;
            let mut delete_layer: Option<usize> = None;
            let mut add_layer_operation: Option<(usize, crate::cam::operations::Operation)> = None;

            for (i, (is_selected, vis_icon, label, _vis)) in layer_display.iter().enumerate() {
                let row_bg = if *is_selected {
                    selected_bg
                } else {
                    egui::Color32::TRANSPARENT
                };

                let row_button_response = egui::Frame::new()
                    .fill(row_bg)
                    .corner_radius(egui::CornerRadius::same(4))
                    .inner_margin(egui::Margin::symmetric(6, 3))
                    .show(ui, |ui| {
                        let row_width = ui.available_width();
                        ui.set_width(row_width);
                        ui.horizontal(|ui| {
                            let visibility_icon = egui::RichText::new(vis_icon.to_string())
                                .font(egui::FontId::new(
                                    15.0,
                                    egui::FontFamily::Name("sidebar_icons".into()),
                                ))
                                .color(row_text);
                            let visibility_response = ui
                                .add_sized(
                                    [22.0, 20.0],
                                    egui::Button::new(visibility_icon).frame(false),
                                )
                                .on_hover_text(if *_vis { "Hide layer" } else { "Show layer" });
                            if visibility_response.clicked() {
                                toggle_vis = Some(i);
                            }
                            let layer_button = egui::Button::new(
                                egui::RichText::new(label).size(13.0).color(row_text),
                            )
                            .selected(*is_selected)
                            .corner_radius(egui::CornerRadius::same(5))
                            .frame(false);
                            let layer_response =
                                ui.add_sized([ui.available_width(), 24.0], layer_button);
                            if layer_response.clicked() {
                                select_layer = Some(i);
                            }
                            layer_response
                        })
                        .inner
                    })
                    .inner;
                row_button_response.context_menu(|ui| {
                    let kind = &app.project.layers[i].kind;
                    match kind {
                        LayerKind::Drill => {
                            if ui.button("Add Drilling").clicked() {
                                add_layer_operation =
                                    Some((i, crate::cam::operations::Operation::drill()));
                                ui.close_menu();
                            }
                        }
                        LayerKind::Gerber
                        | LayerKind::Dxf
                        | LayerKind::Svg
                        | LayerKind::Text
                        | LayerKind::Drawing => {
                            ui.menu_button("Add CAM operation", |ui| {
                                for (label, operation) in [
                                    (
                                        "Isolation Routing",
                                        crate::cam::operations::Operation::isolation(),
                                    ),
                                    (
                                        "Pocket Milling",
                                        crate::cam::operations::Operation::pocket(),
                                    ),
                                    ("Contour Cut", crate::cam::operations::Operation::contour()),
                                ] {
                                    if ui.button(label).clicked() {
                                        add_layer_operation = Some((i, operation));
                                        ui.close_menu();
                                    }
                                }
                            });
                        }
                        LayerKind::Gcode | LayerKind::Placeholder => {}
                    }
                    if !matches!(kind, LayerKind::Gcode | LayerKind::Placeholder) {
                        ui.separator();
                    }
                    if ui.button("Delete layer").clicked() {
                        delete_layer = Some(i);
                        ui.close_menu();
                    }
                });
            }

            // Apply deferred mutations
            if let Some((index, mut operation)) = add_layer_operation {
                if let Some(layer) = app.project.layers.get(index) {
                    app.ui_state.selected_layer = Some(index);
                    operation.source_layer_id = Some(layer.id);
                    app.add_operation(operation);
                }
            }
            if let Some(i) = toggle_vis {
                app.project.layers[i].visible = !app.project.layers[i].visible;
                app.ui_state.project_dirty = true;
                app.scene_sig.clear();
            }
            if let Some(i) = select_layer {
                app.ui_state.selected_layer = Some(i);
                if let Some(layer) = app
                    .project
                    .layers
                    .get(i)
                    .filter(|layer| layer.kind == LayerKind::Text)
                {
                    let input = layer
                        .text_data
                        .as_ref()
                        .map(|data| data.content.clone())
                        .or_else(|| layer.name.strip_prefix("Text: ").map(str::to_owned))
                        .unwrap_or_default();
                    let height = layer
                        .text_data
                        .as_ref()
                        .map(|data| data.height_mm)
                        .or_else(|| {
                            layer
                                .geometry
                                .bounds()
                                .map(|(min, max)| (max[1] - min[1]).max(1.0) as f64)
                        })
                        .unwrap_or(5.0);
                    app.ui_state.text_layer_input = input;
                    app.ui_state.text_layer_height_mm = height;
                    app.ui_state.text_layer_font_style = layer
                        .text_data
                        .as_ref()
                        .map_or(TextFontStyle::default(), |data| data.font_style);
                    app.ui_state.editing_text_layer = Some(i);
                    app.ui_state.show_text_dialog = true;
                    app.ui_state.placing_text_layer = None;
                    app.ui_state.text_resize = None;
                    app.ui_state.text_rotate = None;
                }
            }
            if let Some(i) = delete_layer {
                let layer_id = app.project.layers[i].id;
                app.project.layers.remove(i);
                app.reset_stock_surface();
                for operation in &mut app.project.operations {
                    if operation.source_layer_id == Some(layer_id) {
                        operation.source_layer_id = None;
                    }
                }
                app.ui_state.selected_layer = match app.ui_state.selected_layer {
                    Some(selected) if selected == i => (!app.project.layers.is_empty())
                        .then_some(i.min(app.project.layers.len().saturating_sub(1))),
                    Some(selected) if selected > i => Some(selected - 1),
                    selected => selected,
                };
                app.ui_state.project_dirty = true;
                app.scene_sig.clear();
            }

            if !app.project.models.is_empty() {
                ui.add_space(8.0);
                section_header(ui, "3D MODELS", header_color);
                let model_display: Vec<(bool, bool, String)> = app
                    .project
                    .models
                    .iter()
                    .enumerate()
                    .map(|(index, model)| {
                        (
                            app.ui_state.selected_model == Some(index),
                            model.visible,
                            format!(
                                "{} · {} bodies · {}",
                                model.name,
                                model.bodies.len(),
                                model.source_unit.label()
                            ),
                        )
                    })
                    .collect();
                let mut changed_model_visibility = None;
                let mut select_model = None;
                let mut delete_model = None;
                let mut add_model_operation: Option<(usize, crate::cam::operations::Operation)> =
                    None;
                for (index, (selected, visible, label)) in model_display.iter().enumerate() {
                    let mut child_hit_rects = Vec::with_capacity(2);
                    let row = ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 26.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.add_space(4.0);
                            let visibility_icon = if *visible { ICON_EYE } else { ICON_EYE_OFF };
                            let icon = egui::RichText::new(visibility_icon.to_string())
                                .font(egui::FontId::new(
                                    15.0,
                                    egui::FontFamily::Name("sidebar_icons".into()),
                                ))
                                .color(row_text);
                            let visibility_response = ui
                                .add_sized([22.0, 20.0], egui::Button::new(icon).frame(false))
                                .on_hover_text(if *visible { "Hide model" } else { "Show model" });
                            child_hit_rects.push(visibility_response.rect);
                            if visibility_response.clicked() {
                                changed_model_visibility = Some((index, !*visible));
                            }
                            ui.add_space(4.0);
                            show_model_context_menu(
                                &visibility_response,
                                index,
                                &mut add_model_operation,
                                &mut delete_model,
                            );
                            let model_button_width = (ui.available_width() - 4.0).max(0.0);
                            let model_response = ui.add(
                                egui::Button::new(label)
                                    .selected(*selected)
                                    .corner_radius(egui::CornerRadius::same(3))
                                    .frame(false)
                                    .min_size(egui::vec2(model_button_width, 26.0))
                                    .truncate(),
                            );
                            child_hit_rects.push(model_response.rect);
                            if model_response.clicked() {
                                select_model = Some(index);
                            }
                            show_model_context_menu(
                                &model_response,
                                index,
                                &mut add_model_operation,
                                &mut delete_model,
                            );
                        },
                    );
                    let response = row.response.interact(egui::Sense::click());
                    let pointer_over_child = ctx
                        .input(|input| input.pointer.interact_pos())
                        .is_some_and(|position| {
                            child_hit_rects.iter().any(|rect| rect.contains(position))
                        });
                    if !pointer_over_child {
                        show_model_context_menu(
                            &response,
                            index,
                            &mut add_model_operation,
                            &mut delete_model,
                        );
                    }
                }
                if let Some((index, operation)) = add_model_operation {
                    app.ui_state.selected_model = Some(index);
                    app.ui_state.show_3d_view = true;
                    app.ui_state.request_fit = true;
                    app.project.models[index].visible = true;
                    app.add_operation(operation);
                }
                if let Some((index, visible)) = changed_model_visibility {
                    app.project.models[index].visible = visible;
                    app.ui_state.project_dirty = true;
                }
                if let Some(index) = select_model {
                    app.ui_state.selected_model = Some(index);
                    app.ui_state.request_fit = true;
                }
                let keyboard_is_focused = ctx.wants_keyboard_input();
                let delete_key_pressed =
                    ctx.input(|input| input.key_pressed(egui::Key::Delete) && !keyboard_is_focused);
                if delete_model.is_none() && delete_key_pressed {
                    delete_model = app.ui_state.selected_model;
                }
                if let Some(index) = delete_model.filter(|index| *index < app.project.models.len())
                {
                    app.project.models.remove(index);
                    app.reset_stock_surface();
                    app.ui_state.selected_model = match app.ui_state.selected_model {
                        Some(selected) if selected == index => (!app.project.models.is_empty())
                            .then_some(index.min(app.project.models.len().saturating_sub(1))),
                        Some(selected) if selected > index => Some(selected - 1),
                        selected => selected,
                    };
                    app.ui_state.project_dirty = true;
                    app.ui_state.request_fit = true;
                    app.scene_sig.clear();
                }
            }

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);

            // ── CAM Operations section ────────────────────────────────────
            section_header(ui, "CAM OPERATIONS", header_color);
            let op_count = app.project.operations.len();

            // Extract display data for operations
            let op_display: Vec<(bool, &'static str, String, bool, egui::Color32, bool)> = (0..op_count)
                .map(|i| {
                    let op = &app.project.operations[i];
                    let selected = app.ui_state.selected_operation == Some(i);
                    let icon: &'static str = match op.kind {
                        OperationKind::Isolation => "ISO",
                        OperationKind::Pocket => "POC",
                        OperationKind::Drill => "DRL",
                        OperationKind::HelicalBore => "HEL",
                        OperationKind::Contour => "CON",
                        OperationKind::ModelProfile2D => "2D",
                        OperationKind::Roughing3D => "R3D",
                        OperationKind::FinishRaster3D => "FR3",
                        OperationKind::FinishContour3D => "FC3",
                        OperationKind::FinishWaterline3D => "FW3",
                        OperationKind::Placeholder => "NEW",
                        OperationKind::Custom(_) => "USR",
                    };
                    let [r, g, b] = crate::cam::operations::operation_color(i);
                    let swatch = egui::Color32::from_rgb(
                        (r * 255.0) as u8,
                        (g * 255.0) as u8,
                        (b * 255.0) as u8,
                    );
                    (selected, icon, op.name.clone(), op.generated, swatch, op.visible)
                })
                .collect();

            let mut select_op: Option<usize> = None;
            let mut delete_op: Option<usize> = None;
            let mut set_operation_exclusion: Option<(usize, bool)> = None;
            let mut toggle_operation_visibility: Option<usize> = None;

            for (i, (is_selected, icon, name, generated, swatch, shown)) in
                op_display.iter().enumerate()
            {
                let row_bg = if *is_selected {
                    selected_bg
                } else {
                    egui::Color32::TRANSPARENT
                };
                let row_border = if *is_selected {
                    accent
                } else if dark {
                    egui::Color32::from_rgb(63, 64, 72)
                } else {
                    egui::Color32::from_rgb(196, 198, 210)
                };

                // Where the eye is, so a click on it can be told from a click on the row.
                let mut eye_rect = egui::Rect::NOTHING;
                let response = egui::Frame::new()
                    .fill(row_bg)
                    .stroke(egui::Stroke::new(
                        if *is_selected { 1.2_f32 } else { 0.8 },
                        row_border,
                    ))
                    .corner_radius(egui::CornerRadius::same(4))
                    .inner_margin(egui::Margin::symmetric(6, 3))
                    .show(ui, |ui| {
                        let row_width = ui.available_width();
                        ui.set_width(row_width);
                        ui.horizontal(|ui| {
                            let state_color = if *generated {
                                if dark {
                                    crate::ui::palette::FISHY_HOUSE
                                } else {
                                    egui::Color32::from_rgb(60, 200, 100)
                                }
                            } else {
                                egui::Color32::from_rgb(180, 160, 60)
                            };
                            let state_icon = if *generated {
                                ICON_CHECK
                            } else {
                                ICON_PENDING
                            };
                            let state_text = egui::RichText::new(state_icon.to_string())
                                .font(egui::FontId::new(
                                    11.0,
                                    egui::FontFamily::Name("download_icon".into()),
                                ))
                                .color(state_color);
                            // Eye | colour dot | name (fills the rest) | state.
                            let gap = ui.spacing().item_spacing.x;
                            let eye_width = 22.0_f32;
                            let dot_width = 12.0_f32;
                            let state_width = 14.0_f32;
                            let name_width = (row_width
                                - eye_width
                                - dot_width
                                - state_width
                                - 3.0 * gap)
                                .max(0.0);

                            let eye_icon = egui::RichText::new(
                                (if *shown { ICON_EYE } else { ICON_EYE_OFF }).to_string(),
                            )
                            .font(egui::FontId::new(
                                14.0,
                                egui::FontFamily::Name("sidebar_icons".into()),
                            ))
                            .color(row_text);
                            let eye = ui
                                .add_sized(
                                    [eye_width, 18.0],
                                    egui::Button::new(eye_icon).frame(false),
                                )
                                .on_hover_text(if *shown {
                                    "Hide toolpath"
                                } else {
                                    "Show toolpath"
                                });
                            eye_rect = eye.rect;

                            // Toolpath colour, matching the operation in the viewport
                            // (hollow until its toolpath has been generated; faded
                            // while the toolpath is hidden).
                            let dot_color = if *shown {
                                *swatch
                            } else {
                                swatch.gamma_multiply(0.35)
                            };
                            let (dot_rect, _) = ui.allocate_exact_size(
                                egui::vec2(dot_width, 18.0),
                                egui::Sense::hover(),
                            );
                            if *generated {
                                ui.painter()
                                    .circle_filled(dot_rect.center(), 5.0, dot_color);
                            } else {
                                ui.painter().circle_stroke(
                                    dot_rect.center(),
                                    4.5,
                                    egui::Stroke::new(1.5_f32, dot_color),
                                );
                            }

                            // Left-aligned, truncated name.
                            ui.allocate_ui_with_layout(
                                egui::vec2(name_width, 18.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(format!("{icon} {name}"))
                                                .size(13.0)
                                                .color(row_text),
                                        )
                                        .truncate(),
                                    );
                                },
                            );
                            ui.add_sized([state_width, 18.0], egui::Label::new(state_text));
                        });
                    })
                    .response;

                // The row-wide click area sits above the eye, so tell the two apart
                // by where the pointer was.
                let response = response.interact(egui::Sense::click());
                if response.clicked() {
                    let on_eye = response
                        .interact_pointer_pos()
                        .is_some_and(|pointer| eye_rect.expand(2.0).contains(pointer));
                    if on_eye {
                        toggle_operation_visibility = Some(i);
                    } else {
                        select_op = Some(i);
                    }
                }
                response.context_menu(|ui| {
                    let mut add_to_toolpath =
                        !app.project.operations[i].excluded_from_toolpath;
                    if ui
                        .checkbox(&mut add_to_toolpath, "Add to toolpath")
                        .changed()
                    {
                        set_operation_exclusion = Some((i, !add_to_toolpath));
                    }
                    ui.separator();
                    if ui.button("Delete operation").clicked() {
                        delete_op = Some(i);
                        ui.close_menu();
                    }
                });
            }

            if let Some(i) = select_op {
                app.ui_state.selected_operation = Some(i);
            }
            if let Some(index) = toggle_operation_visibility {
                let operation = &mut app.project.operations[index];
                operation.visible = !operation.visible;
                app.ui_state.project_dirty = true;
                // A hidden operation is also left out of the simulation.
                app.invalidate_simulation();
                app.scene_sig.clear();
            }
            if let Some((index, excluded)) = set_operation_exclusion {
                app.project.operations[index].excluded_from_toolpath = excluded;
                app.invalidate_gcode_and_simulation();
                app.ui_state.project_dirty = true;
                app.scene_sig.clear();
            }
            if let Some(i) = delete_op {
                app.project.operations.remove(i);
                app.ui_state.selected_operation = if app.project.operations.is_empty() {
                    None
                } else {
                    Some(i.min(app.project.operations.len() - 1))
                };
                app.invalidate_gcode_and_simulation();
                app.ui_state.project_dirty = true;
                app.scene_sig.clear();
            }

            if op_count == 0 {
                let empty_state_height = (ui.available_height() - 40.0).max(36.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), empty_state_height),
                    egui::Layout::centered_and_justified(egui::Direction::TopDown),
                    |ui| {
                        ui.label(
                            egui::RichText::new(
                                "Your toolpaths will appear here.\nUse + in the top bar to add an operation.",
                            )
                            .size(12.0)
                            .color(egui::Color32::from_rgb(125, 128, 148)),
                        );
                    },
                );
            }

            // ── Bottom: Units display ─────────────────────────────────────
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Units:")
                            .size(11.0)
                            .color(egui::Color32::from_rgb(120, 120, 140)),
                    );
                    ui.label(
                        egui::RichText::new(app.ui_state.display_units.label())
                            .size(11.0)
                            .color(accent),
                    );
                });
            });
        });
    if app.ui_state.show_text_dialog {
        let mut open = app.ui_state.show_text_dialog;
        let editing_index = app.ui_state.editing_text_layer;
        let mut save_text = false;
        let mut cancel = false;
        egui::Window::new(if editing_index.is_some() {
            "Edit Text"
        } else {
            "Add Text"
        })
        .id(egui::Id::new("add_text_layer_dialog"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label("Text to engrave");
            let response = ui.add_sized(
                [280.0, 24.0],
                egui::TextEdit::singleline(&mut app.ui_state.text_layer_input)
                    .hint_text("Enter text"),
            );
            if app.ui_state.text_layer_input.is_empty() {
                response.request_focus();
            }
            ui.label("Text height");
            ui.horizontal(|ui| {
                for (heading, height) in [
                    ("H1", 12.0),
                    ("H2", 10.0),
                    ("H3", 8.0),
                    ("H4", 6.0),
                    ("H5", 5.0),
                    ("H6", 4.0),
                ] {
                    ui.selectable_value(&mut app.ui_state.text_layer_height_mm, height, heading)
                        .on_hover_text(format!("{height:.0} mm high"));
                }
            });
            ui.label("Font style");
            egui::ComboBox::from_id_salt("text_font_style")
                .selected_text(match app.ui_state.text_layer_font_style {
                    TextFontStyle::SingleLine => "Segmented",
                    TextFontStyle::Stroke => "Single stroke",
                    TextFontStyle::Bold => "Bold",
                    TextFontStyle::Italic => "Italic",
                    TextFontStyle::BoldItalic => "Bold italic",
                })
                .show_ui(ui, |ui| {
                    for (style, label) in [
                        (TextFontStyle::SingleLine, "Segmented"),
                        (TextFontStyle::Stroke, "Single stroke"),
                        (TextFontStyle::Bold, "Bold"),
                        (TextFontStyle::Italic, "Italic"),
                        (TextFontStyle::BoldItalic, "Bold italic"),
                    ] {
                        ui.selectable_value(&mut app.ui_state.text_layer_font_style, style, label);
                    }
                });
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !app.ui_state.text_layer_input.trim().is_empty(),
                        egui::Button::new(if editing_index.is_some() {
                            "Save Changes"
                        } else {
                            "Add to Stock"
                        }),
                    )
                    .clicked()
                {
                    save_text = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        app.ui_state.show_text_dialog = open && !cancel;
        if cancel || !open {
            app.ui_state.editing_text_layer = None;
        }
        if save_text {
            let text = app.ui_state.text_layer_input.trim().to_owned();
            let height = app.ui_state.text_layer_height_mm;
            let font_style = app.ui_state.text_layer_font_style;
            if let Some(index) = editing_index.filter(|&index| index < app.project.layers.len()) {
                let layer = &mut app.project.layers[index];
                let center = layer
                    .geometry
                    .bounds()
                    .map(|(min, max)| [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5])
                    .unwrap_or([0.0, 0.0]);
                let rotation = layer
                    .text_data
                    .as_ref()
                    .map_or(0.0, |data| data.rotation_degrees);
                layer.geometry = make_text_geometry(&text, height, center, rotation, font_style);
                layer.name = format!("Text: {text}");
                layer.text_data = Some(crate::workspace::layer::TextLayerData {
                    content: text,
                    height_mm: height,
                    font_style,
                    rotation_degrees: rotation,
                });
                app.ui_state.editing_text_layer = None;
            } else {
                let setup = &app.project.setup;
                let cell = height / 7.0;
                let width = text.chars().count().saturating_sub(1) as f64 * cell * 6.0
                    + text.chars().count() as f64 * cell;
                let origin = [
                    setup.stock_origin_mm[0] + (setup.stock_size_mm[0] - width) * 0.5,
                    setup.stock_origin_mm[1] + (setup.stock_size_mm[1] - height) * 0.5,
                ];
                let mut geometry = crate::workspace::object::Geometry::default();
                add_styled_text(&mut geometry, &text, origin, height, font_style);
                let mut layer = Layer::new_text(format!("Text: {text}"), geometry);
                layer.text_data = Some(crate::workspace::layer::TextLayerData {
                    content: text,
                    height_mm: height,
                    font_style,
                    rotation_degrees: 0.0,
                });
                app.project.layers.push(layer);
                app.invalidate_gcode_and_simulation();
                let index = app.project.layers.len() - 1;
                app.ui_state.selected_layer = Some(index);
                app.ui_state.placing_text_layer = Some(index);
                app.ui_state.text_placement_offset = [0.0; 2];
            }
            app.ui_state.text_layer_input.clear();
            app.ui_state.show_text_dialog = false;
            app.ui_state.project_dirty = true;
            app.scene_sig.clear();
        }
    }

    if app.ui_state.show_drawing_dialog {
        let mut open = true;
        let mut place = false;
        let mut cancel = false;
        egui::Window::new("Draw Design")
            .id(egui::Id::new("draw_design_dialog"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("Sketch your design, then place it in the viewport.");
                ui.horizontal(|ui| {
                    ui.label("Width (mm)");
                    ui.add(
                        egui::DragValue::new(&mut app.ui_state.drawing_width_mm).range(1.0..=500.0),
                    );
                    ui.label("Height (mm)");
                    ui.add(
                        egui::DragValue::new(&mut app.ui_state.drawing_height_mm)
                            .range(1.0..=500.0),
                    );
                });

                let (canvas, response) =
                    ui.allocate_exact_size(egui::vec2(420.0, 260.0), egui::Sense::drag());
                let painter = ui.painter_at(canvas);
                painter.rect_filled(canvas, 4.0, egui::Color32::from_rgb(25, 29, 39));
                painter.rect_stroke(
                    canvas,
                    4.0,
                    egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(75, 87, 110)),
                    egui::StrokeKind::Inside,
                );
                let pointer = response.interact_pointer_pos();
                if response.drag_started() {
                    app.ui_state.drawing_strokes.push(Vec::new());
                }
                if response.dragged() {
                    if let Some(pointer) = pointer {
                        let normalized = [
                            ((pointer.x - canvas.left()) / canvas.width()).clamp(0.0, 1.0),
                            ((pointer.y - canvas.top()) / canvas.height()).clamp(0.0, 1.0),
                        ];
                        if let Some(stroke) = app.ui_state.drawing_strokes.last_mut() {
                            if stroke.last().is_none_or(|last| {
                                (last[0] - normalized[0]).hypot(last[1] - normalized[1]) > 0.002
                            }) {
                                stroke.push(normalized);
                            }
                        }
                    }
                }
                for stroke in &app.ui_state.drawing_strokes {
                    for pair in stroke.windows(2) {
                        let point = |p: [f32; 2]| {
                            egui::pos2(
                                canvas.left() + p[0] * canvas.width(),
                                canvas.top() + p[1] * canvas.height(),
                            )
                        };
                        painter.line_segment(
                            [point(pair[0]), point(pair[1])],
                            egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(90, 195, 245)),
                        );
                    }
                }
                ui.horizontal(|ui| {
                    if ui.button("Undo stroke").clicked() {
                        app.ui_state.drawing_strokes.pop();
                    }
                    if ui.button("Clear").clicked() {
                        app.ui_state.drawing_strokes.clear();
                    }
                    ui.add_space(12.0);
                    if ui
                        .add_enabled(
                            app.ui_state
                                .drawing_strokes
                                .iter()
                                .any(|stroke| stroke.len() > 1),
                            egui::Button::new("Place in Viewport"),
                        )
                        .clicked()
                    {
                        place = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        app.ui_state.show_drawing_dialog = open && !place && !cancel;
        if place {
            let width = app.ui_state.drawing_width_mm.max(1.0);
            let height = app.ui_state.drawing_height_mm.max(1.0);
            let mut geometry = crate::workspace::object::Geometry::default();
            for stroke in &app.ui_state.drawing_strokes {
                for pair in stroke.windows(2) {
                    for point in pair {
                        geometry
                            .lines
                            .push([(point[0] - 0.5) * width, (0.5 - point[1]) * height]);
                    }
                }
            }
            let mut layer = Layer::new_drawing("Hand drawn design".into(), geometry);
            layer.color = crate::workspace::layer::palette_color(
                &LayerKind::Drawing,
                app.project.layers.len(),
            );
            app.project.layers.push(layer);
            app.invalidate_gcode_and_simulation();
            let index = app.project.layers.len() - 1;
            app.ui_state.selected_layer = Some(index);
            app.ui_state.placing_text_layer = Some(index);
            app.ui_state.text_placement_offset = [0.0; 2];
            app.ui_state.project_dirty = true;
            app.ui_state.status_message = "Move design and double-click to place it".into();
            app.scene_sig.clear();
        }
    }
}

fn make_text_geometry(
    text: &str,
    height_mm: f64,
    center: [f32; 2],
    rotation_degrees: f64,
    font_style: TextFontStyle,
) -> crate::workspace::object::Geometry {
    let cell = height_mm / 7.0;
    let width = text
        .chars()
        .map(|character| if character.is_whitespace() { 4.0 } else { 6.0 })
        .sum::<f64>()
        * cell;
    let origin = [
        center[0] as f64 - width * 0.5,
        center[1] as f64 - height_mm * 0.5,
    ];
    let mut geometry = crate::workspace::object::Geometry::default();
    add_styled_text(&mut geometry, text, origin, height_mm, font_style);

    let angle = rotation_degrees.to_radians() as f32;
    let (sin, cos) = angle.sin_cos();
    for point in geometry
        .lines
        .iter_mut()
        .chain(geometry.triangles.iter_mut())
    {
        let x = point[0] - center[0];
        let y = point[1] - center[1];
        point[0] = center[0] + x * cos - y * sin;
        point[1] = center[1] + x * sin + y * cos;
    }
    geometry
}

fn add_styled_text(
    geometry: &mut crate::workspace::object::Geometry,
    text: &str,
    origin: [f64; 2],
    height_mm: f64,
    font_style: TextFontStyle,
) {
    if font_style == TextFontStyle::Stroke {
        geometry.add_stroke_text(text, origin, height_mm);
    } else {
        geometry.add_text(text, origin, height_mm);
        apply_text_font_style(geometry, height_mm, font_style);
    }
}

fn apply_text_font_style(
    geometry: &mut crate::workspace::object::Geometry,
    height_mm: f64,
    font_style: TextFontStyle,
) {
    let cell = (height_mm / 7.0) as f32;
    if matches!(font_style, TextFontStyle::Bold | TextFontStyle::BoldItalic) {
        let offset = cell * 0.14;
        let original = geometry.lines.clone();
        for point in original {
            geometry.lines.push([point[0], point[1] + offset]);
            geometry.lines.push([point[0], point[1] - offset]);
        }
    }
    if matches!(
        font_style,
        TextFontStyle::Italic | TextFontStyle::BoldItalic
    ) {
        let Some((min, max)) = geometry.bounds() else {
            return;
        };
        let center_y = (min[1] + max[1]) * 0.5;
        for point in geometry
            .lines
            .iter_mut()
            .chain(geometry.triangles.iter_mut())
        {
            point[0] += (point[1] - center_y) * 0.22;
        }
    }
}

fn section_header(ui: &mut egui::Ui, title: &str, color: egui::Color32) {
    ui.label(egui::RichText::new(title).size(10.0).color(color).strong());
}

fn show_model_context_menu(
    response: &egui::Response,
    index: usize,
    add_model_operation: &mut Option<(usize, crate::cam::operations::Operation)>,
    delete_model: &mut Option<usize>,
) {
    response.context_menu(|ui| {
        ui.menu_button("Add CAM operation", |ui| {
            for (label, operation) in [
                (
                    "STEP Face Profile",
                    crate::cam::operations::Operation::model_profile_2d(),
                ),
                (
                    "3D Stock Roughing",
                    crate::cam::operations::Operation::roughing_3d(),
                ),
                (
                    "3D Raster Finish",
                    crate::cam::operations::Operation::finish_raster_3d(),
                ),
                (
                    "3D Contour-Parallel Finish",
                    crate::cam::operations::Operation::finish_contour_3d(),
                ),
                (
                    "3D Waterline Finish",
                    crate::cam::operations::Operation::finish_waterline_3d(),
                ),
            ] {
                if ui.button(label).clicked() {
                    *add_model_operation = Some((index, operation));
                    ui.close_menu();
                }
            }
        });
        ui.separator();
        if ui.button("Delete model").clicked() {
            *delete_model = Some(index);
            ui.close_menu();
        }
    });
}
