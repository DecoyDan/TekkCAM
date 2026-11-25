use crate::app::TekkCamApp;
use eframe::egui;

const MUTED: egui::Color32 = egui::Color32::from_rgb(100, 100, 120);

pub fn show(ctx: &egui::Context, app: &mut TekkCamApp) {
    let accent = if app.ui_state.theme == crate::app::AppTheme::Dark {
        crate::ui::palette::SWISS_PLUM
    } else {
        crate::ui::palette::LIGHT_ACCENT_BLUE
    };
    let mut project_changed = false;
    let model_bounds = app
        .ui_state
        .selected_model
        .and_then(|i| app.project.models.get(i).filter(|model| model.visible))
        .map(|m| m.transformed_bounds())
        .or_else(|| {
            app.project
                .models
                .iter()
                .find(|model| model.visible)
                .map(|m| m.transformed_bounds())
        });
    let panel_bg = if app.ui_state.theme == crate::app::AppTheme::Light {
        egui::Color32::from_rgb(228, 229, 241)
    } else {
        crate::ui::palette::MYSTERIOUS_DEPTHS
    };
    egui::SidePanel::right("properties")
        .resizable(true)
        .default_width(300.0)
        .min_width(230.0)
        .max_width(520.0)
        .frame(
            egui::Frame::new()
                .fill(panel_bg)
                .inner_margin(egui::Margin::same(10)),
        )
        .show(ctx, |ui| {
            // Keep the frame at the width selected by the resize handle. Without
            // this, the content's intrinsic width can collapse it after a drag.
            ui.set_min_width(ui.max_rect().width());

            let dark = app.ui_state.theme == crate::app::AppTheme::Dark;
            let tab_bar_bg = if dark {
                crate::ui::palette::MYSTERIOUS_DEPTHS
            } else {
                egui::Color32::from_rgb(215, 217, 229)
            };
            let inactive_text = if dark {
                egui::Color32::from_rgb(112, 116, 132)
            } else {
                egui::Color32::from_rgb(117, 120, 139)
            };
            egui::Frame::new()
                .fill(tab_bar_bg)
                .corner_radius(egui::CornerRadius::same(9))
                .inner_margin(egui::Margin::same(3))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let tab_width = ui.available_width() * 0.5;
                    ui.horizontal(|ui| {
                        for (is_console, label) in
                            [(false, "Properties"), (true, "Console / G-code")]
                        {
                            let active = app.ui_state.show_console == is_console;
                            let button = egui::Button::new(
                                egui::RichText::new(label)
                                    .size(13.0)
                                    .strong()
                                    .color(if active {
                                        egui::Color32::WHITE
                                    } else {
                                        inactive_text
                                    }),
                            )
                            .fill(if active {
                                crate::ui::palette::SWISS_PLUM
                            } else {
                                egui::Color32::TRANSPARENT
                            })
                            .stroke(egui::Stroke::NONE)
                            .corner_radius(egui::CornerRadius::same(7));
                            if ui.add_sized([tab_width, 34.0], button).clicked() {
                                app.ui_state.show_console = is_console;
                            }
                        }
                    });
                });
            ui.separator();

            if app.ui_state.show_console {
                crate::ui::console::show_in_sidebar(ui, ctx, app);
                return;
            }

            if !app.has_properties_content() {
                ui.add_space(16.0);
                ui.label(
                    egui::RichText::new("Select a layer or operation to edit its properties.")
                        .color(MUTED)
                        .size(13.0),
                );
                return;
            }

            egui::ScrollArea::vertical()
                .id_salt("properties_content")
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.set_max_width((ui.available_width() - 8.0).max(0.0));
                    // ── Layer properties ──────────────────────────────────────────
                    if let Some(idx) = app.ui_state.selected_layer {
                        if idx < app.project.layers.len() {
                            ui.heading("LAYER PROPERTIES");
                            ui.vertical(|ui| {
                                ui.add_space(6.0);

                                let layer = &mut app.project.layers[idx];

                                prop_row(ui, "Name", |ui| {
                                    project_changed |=
                                        ui.text_edit_singleline(&mut layer.name).changed();
                                });

                                prop_row(ui, "Kind", |ui| {
                                    ui.label(
                                        egui::RichText::new(format!("{:?}", layer.kind))
                                            .color(accent),
                                    );
                                });

                                prop_row(ui, "Visible", |ui| {
                                    project_changed |=
                                        ui.checkbox(&mut layer.visible, "").changed();
                                });

                                prop_row(ui, "File", |ui| {
                                    let file_path = layer.file_path.as_deref();
                                    let file_name = file_path
                                        .and_then(|path| std::path::Path::new(path).file_name())
                                        .and_then(|name| name.to_str())
                                        .unwrap_or("(not loaded)");
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(file_name).size(11.0).color(MUTED),
                                        )
                                        .truncate(),
                                    )
                                    .on_hover_text(file_path.unwrap_or("(not loaded)"));
                                });

                                ui.add_space(12.0);
                                ui.separator();
                                ui.add_space(8.0);
                            });
                        }
                    }

                    // ── Operation properties ──────────────────────────────────────
                    if let Some(idx) = app.ui_state.selected_operation {
                        if idx < app.project.operations.len() {
                            let units = app.ui_state.display_units;
                            ui.heading("OPERATION PROPERTIES");
                            ui.vertical(|ui| {
                                ui.add_space(6.0);

                                let op = &mut app.project.operations[idx];

                                prop_row(ui, "Name", |ui| {
                                    project_changed |=
                                        ui.text_edit_singleline(&mut op.name).changed();
                                });

                                prop_row(ui, "Type", |ui| {
                                    ui.label(
                                        egui::RichText::new(format!("{:?}", op.kind)).color(accent),
                                    );
                                });
                                if op.auto_planned {
                                    prop_row(ui, "Planned", |ui| {
                                        let detail = match (&op.slope_window, op.bores.len()) {
                                            (Some([lo, hi]), _) => {
                                                format!("Auto plan · slopes {lo:.0}°–{hi:.0}°")
                                            }
                                            (None, holes) if holes > 0 => {
                                                format!("Auto plan · {holes} hole(s), helical")
                                            }
                                            _ => "Auto plan".to_string(),
                                        };
                                        ui.label(egui::RichText::new(detail).color(MUTED));
                                    });
                                }

                                if matches!(
                                    &op.kind,
                                    crate::cam::operations::OperationKind::ModelProfile2D
                                ) {
                                    let model_index = app
                                        .ui_state
                                        .selected_model
                                        .filter(|index| {
                                            app.project
                                                .models
                                                .get(*index)
                                                .is_some_and(|model| model.visible)
                                        })
                                        .or_else(|| {
                                            app.project
                                                .models
                                                .iter()
                                                .position(|model| model.visible)
                                        });
                                    if let Some(model) =
                                        model_index.and_then(|index| app.project.models.get(index))
                                    {
                                        let mut face_ids: Vec<u64> = model
                                            .bodies
                                            .iter()
                                            .flat_map(|body| body.triangle_face_ids.iter().copied())
                                            .collect();
                                        face_ids.sort_unstable();
                                        face_ids.dedup();
                                        ui.label(format!("STEP model: {}", model.name));
                                        let automatic_faces =
                                            crate::cam::model_2d::automatic_profile_faces(model);
                                        prop_row(ui, "Face selection", |ui| {
                                            egui::ComboBox::from_id_salt((
                                                "face_selection_mode",
                                                idx,
                                            ))
                                            .selected_text(if op.manual_face_selection {
                                                "Manual"
                                            } else {
                                                "Automatic"
                                            })
                                            .show_ui(
                                                ui,
                                                |ui| {
                                                    if ui
                                                        .selectable_value(
                                                            &mut op.manual_face_selection,
                                                            false,
                                                            "Automatic",
                                                        )
                                                        .changed()
                                                    {
                                                        project_changed = true;
                                                    }
                                                    if ui
                                                        .selectable_value(
                                                            &mut op.manual_face_selection,
                                                            true,
                                                            "Manual",
                                                        )
                                                        .changed()
                                                    {
                                                        if op.source_face_ids.is_empty() {
                                                            op.source_face_ids
                                                                .clone_from(&automatic_faces);
                                                        }
                                                        project_changed = true;
                                                    }
                                                },
                                            );
                                        });
                                        if !op.manual_face_selection {
                                            ui.label(format!(
                                                "Automatic top-view outline · {} source faces",
                                                automatic_faces.len(),
                                            ));
                                        }
                                        if op.manual_face_selection {
                                            ui.label(format!(
                                                "Source faces · {} selected",
                                                op.source_face_ids.len()
                                            ));
                                            ui.add_space(4.0);
                                            let all_selected = !face_ids.is_empty()
                                                && face_ids
                                                    .iter()
                                                    .all(|id| op.source_face_ids.contains(id));
                                            ui.horizontal(|ui| {
                                                if ui
                                                    .add_enabled(
                                                        !face_ids.is_empty() && !all_selected,
                                                        egui::Button::new("Select all"),
                                                    )
                                                    .clicked()
                                                {
                                                    op.source_face_ids.clone_from(&face_ids);
                                                    project_changed = true;
                                                }
                                                if ui
                                                    .add_enabled(
                                                        !op.source_face_ids.is_empty(),
                                                        egui::Button::new("Clear"),
                                                    )
                                                    .clicked()
                                                {
                                                    op.source_face_ids.clear();
                                                    project_changed = true;
                                                }
                                            });
                                            egui::Frame::group(ui.style()).show(ui, |ui| {
                                                ui.set_width(ui.available_width());
                                                ui.set_max_height(180.0);
                                                egui::ScrollArea::vertical()
                                                    .id_salt(("step_face_list", idx))
                                                    .auto_shrink([false, false])
                                                    .show(ui, |ui| {
                                                        for face_id in face_ids {
                                                            let mut selected = op
                                                                .source_face_ids
                                                                .contains(&face_id);
                                                            if ui
                                                                .checkbox(
                                                                    &mut selected,
                                                                    format!("Face {face_id}"),
                                                                )
                                                                .changed()
                                                            {
                                                                if selected {
                                                                    op.source_face_ids
                                                                        .push(face_id);
                                                                } else {
                                                                    op.source_face_ids.retain(
                                                                        |id| *id != face_id,
                                                                    );
                                                                }
                                                                project_changed = true;
                                                            }
                                                        }
                                                    });
                                            });
                                        }
                                    } else {
                                        ui.label(
                                            egui::RichText::new(
                                                "Import and select a STEP model first.",
                                            )
                                            .color(MUTED),
                                        );
                                    }
                                } else if !matches!(
                                    &op.kind,
                                    crate::cam::operations::OperationKind::Roughing3D
                                        | crate::cam::operations::OperationKind::FinishRaster3D
                                        | crate::cam::operations::OperationKind::FinishContour3D
                                        | crate::cam::operations::OperationKind::FinishWaterline3D
                                ) && !(op.kind
                                    == crate::cam::operations::OperationKind::HelicalBore
                                    && !op.bores.is_empty())
                                {
                                    let sources: Vec<(u64, String, bool)> = app
                                        .project
                                        .layers
                                        .iter()
                                        .map(|layer| (layer.id, layer.name.clone(), layer.visible))
                                        .collect();
                                    let selected_text = op
                                        .source_layer_id
                                        .and_then(|id| sources.iter().find(|source| source.0 == id))
                                        .map(|source| source.1.as_str())
                                        .unwrap_or("Auto (first visible layer)");
                                    prop_row(ui, "Geometry source", |ui| {
                                        egui::ComboBox::from_id_salt(("operation_source", idx))
                                            .selected_text(selected_text)
                                            .show_ui(ui, |ui| {
                                                if ui
                                                    .selectable_value(
                                                        &mut op.source_layer_id,
                                                        None,
                                                        "Auto (first visible layer)",
                                                    )
                                                    .changed()
                                                {
                                                    project_changed = true;
                                                }
                                                for (id, name, visible) in &sources {
                                                    if ui
                                                        .selectable_value(
                                                            &mut op.source_layer_id,
                                                            Some(*id),
                                                            format!(
                                                                "{}{}",
                                                                name,
                                                                if *visible {
                                                                    ""
                                                                } else {
                                                                    " (hidden)"
                                                                }
                                                            ),
                                                        )
                                                        .changed()
                                                    {
                                                        project_changed = true;
                                                    }
                                                }
                                            });
                                    });
                                }

                                ui.add_space(8.0);
                                ui.heading("TOOL");
                                ui.vertical(|ui| {
                                    ui.add_space(4.0);

                                    prop_row(ui, &format!("Diameter ({})", units.label()), |ui| {
                                        project_changed |= unit_drag(
                                            ui,
                                            &mut op.tool.diameter_mm,
                                            units,
                                            0.01,
                                            0.01,
                                            25.0,
                                        );
                                    });

                                    prop_row(
                                        ui,
                                        &format!("Cutting Feedrate ({}/min)", units.label()),
                                        |ui| {
                                            project_changed |= unit_drag(
                                                ui,
                                                &mut op.tool.feed_rate_mm_min,
                                                units,
                                                1.0,
                                                10.0,
                                                5000.0,
                                            );
                                        },
                                    );

                                    prop_row(
                                        ui,
                                        &format!("Plunge Feedrate ({}/min)", units.label()),
                                        |ui| {
                                            project_changed |= unit_drag(
                                                ui,
                                                &mut op.tool.plunge_rate_mm_min,
                                                units,
                                                1.0,
                                                10.0,
                                                2000.0,
                                            );
                                        },
                                    );

                                    prop_row(ui, "Spindle Speed (RPM)", |ui| {
                                        project_changed |= ui
                                            .add(
                                                egui::DragValue::new(&mut op.tool.spindle_rpm)
                                                    .speed(50.0)
                                                    .range(0.0..=30000.0),
                                            )
                                            .changed();
                                    });

                                    prop_row(ui, "Tool shape", |ui| {
                                        egui::ComboBox::from_id_salt("operation_tool_shape")
                                            .selected_text(format!("{:?}", op.tool.shape))
                                            .show_ui(ui, |ui| {
                                                for shape in [
                                                    crate::cam::tool::ToolShape::Flat,
                                                    crate::cam::tool::ToolShape::BallNose,
                                                    crate::cam::tool::ToolShape::BullNose,
                                                ] {
                                                    project_changed |= ui
                                                        .selectable_value(
                                                            &mut op.tool.shape,
                                                            shape,
                                                            format!("{:?}", shape),
                                                        )
                                                        .changed();
                                                }
                                            });
                                    });
                                    for (label, value) in [
                                        ("Flute length", &mut op.tool.flute_length_mm),
                                        ("Stickout", &mut op.tool.stickout_mm),
                                        ("Corner radius", &mut op.tool.corner_radius_mm),
                                        ("Holder clearance", &mut op.tool.holder_clearance_mm),
                                    ] {
                                        prop_row(
                                            ui,
                                            &format!("{} ({})", label, units.label()),
                                            |ui| {
                                                project_changed |=
                                                    unit_drag(ui, value, units, 0.1, 0.0, 300.0);
                                            },
                                        );
                                    }
                                });

                                ui.add_space(8.0);
                                ui.heading("CUT PARAMETERS");
                                ui.vertical(|ui| {
                                    ui.add_space(4.0);

                                    let is_roughing = matches!(
                                        &op.kind,
                                        crate::cam::operations::OperationKind::Roughing3D
                                    );
                                    let is_3d_machining = is_roughing
                                        || matches!(
                                    &op.kind,
                                    crate::cam::operations::OperationKind::FinishRaster3D
                                        | crate::cam::operations::OperationKind::FinishContour3D
                                        | crate::cam::operations::OperationKind::FinishWaterline3D
                                );
                                    if is_3d_machining {
                                        ui.label(
                                            egui::RichText::new(
                                                "Pass spacing uses the project stock setup.",
                                            )
                                            .size(11.0)
                                            .color(MUTED),
                                        );
                                    } else {
                                        prop_row(
                                            ui,
                                            &format!("Maximum Stepdown ({})", units.label()),
                                            |ui| {
                                                project_changed |= unit_drag(
                                                    ui,
                                                    &mut op.depth_per_pass_mm,
                                                    units,
                                                    0.01,
                                                    0.01,
                                                    10.0,
                                                );
                                            },
                                        );

                                        prop_row(
                                            ui,
                                            &format!("Cut Depth ({})", units.label()),
                                            |ui| {
                                                project_changed |= unit_drag(
                                                    ui,
                                                    &mut op.total_depth_mm,
                                                    units,
                                                    0.01,
                                                    0.01,
                                                    50.0,
                                                );
                                            },
                                        );
                                    }

                                    let margin_label = if is_roughing {
                                        "Stock to Leave"
                                    } else if is_3d_machining {
                                        "Stock to Leave"
                                    } else {
                                        "Stock to Leave"
                                    };
                                    prop_row(
                                        ui,
                                        &format!("{} ({})", margin_label, units.label()),
                                        |ui| {
                                            project_changed |= unit_drag(
                                                ui,
                                                &mut op.margin_mm,
                                                units,
                                                0.01,
                                                0.0,
                                                5.0,
                                            );
                                        },
                                    );

                                    use crate::cam::operations::OperationKind as Kind;
                                    if matches!(
                                        &op.kind,
                                        Kind::Isolation | Kind::Contour | Kind::ModelProfile2D
                                    ) {
                                        prop_row(ui, "Ramp angle (deg, 0 = plunge)", |ui| {
                                            project_changed |= ui
                                                .add(
                                                    egui::DragValue::new(&mut op.ramp_angle_deg)
                                                        .speed(0.1)
                                                        .range(0.0..=45.0),
                                                )
                                                .changed();
                                        });
                                    }
                                    if op.kind == Kind::Pocket {
                                        prop_row(ui, "Stepover (% of tool diameter)", |ui| {
                                            project_changed |= ui
                                                .add(
                                                    egui::DragValue::new(&mut op.stepover_ratio)
                                                        .speed(0.01)
                                                        .range(0.05..=1.0),
                                                )
                                                .changed();
                                        });
                                    }
                                    if op.kind == Kind::Drill {
                                        prop_row(
                                            ui,
                                            &format!("Pecking Depth ({})", units.label()),
                                            |ui| {
                                                project_changed |= unit_drag(
                                                    ui,
                                                    &mut op.peck_depth_mm,
                                                    units,
                                                    0.05,
                                                    0.0,
                                                    50.0,
                                                );
                                            },
                                        );
                                        prop_row(
                                            ui,
                                            &format!("Chip-break retract ({})", units.label()),
                                            |ui| {
                                                project_changed |= unit_drag(
                                                    ui,
                                                    &mut op.chip_break_mm,
                                                    units,
                                                    0.05,
                                                    0.0,
                                                    50.0,
                                                );
                                            },
                                        );
                                    }
                                });

                                ui.add_space(12.0);

                                // Generate button
                                let button_text = if app.generation_pending {
                                    "◌  Generating Toolpath…"
                                } else {
                                    "▶  Generate Toolpath"
                                };
                                let btn = egui::Button::new(
                                    egui::RichText::new(button_text).color(egui::Color32::WHITE),
                                )
                                .fill(
                                    if app.ui_state.theme == crate::app::AppTheme::Dark {
                                        crate::ui::palette::SWISS_PLUM
                                    } else {
                                        egui::Color32::from_rgb(50, 100, 200)
                                    },
                                );

                                if ui
                                    .add_enabled_ui(!app.generation_pending, |ui| {
                                        ui.add_sized([ui.available_width(), 32.0], btn)
                                    })
                                    .inner
                                    .clicked()
                                {
                                    app.generate_operation(idx);
                                }

                                ui.add_space(4.0);

                                let export_btn = egui::Button::new(
                                    egui::RichText::new("📤  Export G-code").size(12.0),
                                );
                                if ui
                                    .add_sized([ui.available_width(), 28.0], export_btn)
                                    .clicked()
                                {
                                    crate::ui::console::export_gcode(ctx, app);
                                }
                            });
                        }
                    }
                });
        });
    if project_changed {
        app.ui_state.project_dirty = true;
        app.scene_sig.clear();
    }
}

pub fn show_fit_model_dialog(ctx: &egui::Context, app: &mut TekkCamApp) {
    if !app.ui_state.show_fit_model_dialog {
        app.ui_state.fit_pending_transform = None;
        return;
    }
    let model_index = app
        .ui_state
        .selected_model
        .filter(|index| {
            app.project
                .models
                .get(*index)
                .is_some_and(|model| model.visible)
        })
        .or_else(|| app.project.models.iter().position(|model| model.visible));
    let Some(model_index) = model_index else {
        app.ui_state.show_fit_model_dialog = false;
        app.ui_state.fit_pending_transform = None;
        return;
    };
    let current = app.project.models[model_index].transform;
    // The orientation being tried; the model itself is untouched until Apply.
    let mut pending = match app.ui_state.fit_pending_transform {
        Some((index, transform)) if index == model_index => transform,
        _ => current,
    };
    let tool_clearance = app
        .project
        .operations
        .iter()
        .map(|operation| operation.tool.diameter_mm * 0.5 + operation.tool.holder_clearance_mm)
        .fold(
            app.project.default_tool.diameter_mm * 0.5
                + app.project.default_tool.holder_clearance_mm,
            f64::max,
        );
    let preview = app.project.models[model_index].stock_envelope_with_transform(
        &app.project.setup,
        tool_clearance,
        pending,
    );
    let dark = app.ui_state.theme == crate::app::AppTheme::Dark;
    let units = app.ui_state.display_units;
    let mut open = true;
    let mut apply = false;
    let mut cancel = false;
    let mut turn: Option<(usize, f64)> = None;
    let mut revert = false;
    egui::Window::new("Fit stock to model")
        .id(egui::Id::new("fit_stock_to_model_dialog"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label(
                "Turn the model so the face you want to machine points up (+Z, the tool side), check the preview, then fit the stock. The model turns about its own centre; scale is unchanged.",
            );
            ui.add_space(6.0);
            let stock = preview
                .as_ref()
                .ok()
                .map(|envelope| (envelope.stock_origin_mm, envelope.stock_size_mm));
            crate::ui::orientation_preview::show(
                ui,
                &app.project.models[model_index],
                pending,
                stock,
                &mut app.ui_state.fit_preview_orbit,
                dark,
                230.0,
            );
            ui.add_space(6.0);
            for (axis, name) in ["X", "Y", "Z"].into_iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(format!("About {name}"));
                    if ui.button("-90°").clicked() {
                        turn = Some((axis, -90.0));
                    }
                    if ui.button("+90°").clicked() {
                        turn = Some((axis, 90.0));
                    }
                    if ui
                        .button(format!("Flip {name} 180°"))
                        .on_hover_text("Turn the model over about this axis")
                        .clicked()
                    {
                        turn = Some((axis, 180.0));
                    }
                });
            }
            if ui
                .add_enabled(pending != current, egui::Button::new("Revert orientation"))
                .clicked()
            {
                revert = true;
            }
            ui.add_space(6.0);
            match &preview {
                Ok(envelope) => {
                    let factor = units.factor();
                    let size = envelope.stock_size_mm;
                    ui.label(format!(
                        "New stock size: {:.2} × {:.2} × {:.2} {}",
                        size[0] / factor,
                        size[1] / factor,
                        size[2] / factor,
                        units.label()
                    ));
                    ui.label(format!(
                        "XY clearance: {:.2} {} · Z margin: {:.2} {}",
                        envelope.xy_margin_mm / factor,
                        units.label(),
                        envelope.z_margin_mm / factor,
                        units.label()
                    ));
                }
                Err(error) => {
                    ui.colored_label(egui::Color32::from_rgb(220, 80, 80), error);
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(preview.is_ok(), egui::Button::new("Apply stock size"))
                    .clicked()
                {
                    apply = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    if let Some((axis, degrees)) = turn {
        let bounds = app.project.models[model_index].bounds_with_transform(pending);
        let pivot: [f64; 3] =
            std::array::from_fn(|k| (bounds[0][k] as f64 + bounds[1][k] as f64) * 0.5);
        pending = pending.rotated_about_world_axis(axis, degrees, pivot);
    }
    if revert {
        pending = current;
    }
    if cancel {
        open = false;
    }
    app.ui_state.show_fit_model_dialog = open;
    app.ui_state.fit_pending_transform = open.then_some((model_index, pending));
    if apply {
        // Recompute for the final orientation (a turn and Apply cannot share a frame).
        let envelope = app.project.models[model_index].stock_envelope_with_transform(
            &app.project.setup,
            tool_clearance,
            pending,
        );
        if let Ok(envelope) = envelope {
            app.project.models[model_index].transform = pending;
            app.project.setup.stock_origin_mm = envelope.stock_origin_mm;
            app.project.setup.stock_size_mm = envelope.stock_size_mm;
            invalidate_model_toolpaths(app);
            app.ui_state.project_dirty = true;
            app.ui_state.request_fit = true;
            app.scene_sig.clear();
            app.ui_state.show_fit_model_dialog = false;
            app.ui_state.fit_pending_transform = None;
        }
    } else if turn.is_some() || revert {
        ctx.request_repaint();
    }
}

/// Options for the automatic operation planner.
pub fn show_auto_plan_dialog(ctx: &egui::Context, app: &mut TekkCamApp) {
    if !app.ui_state.show_auto_plan_dialog {
        return;
    }
    let mut open = true;
    let mut run = false;
    let mut setup_changed = false;
    egui::Window::new("Auto plan operations")
        .id(egui::Id::new("auto_plan_dialog"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label(
                "Looks at the model in its stock and picks the operations it needs: roughing, helical bores and finishing. Every operation gets its own colour in the viewport.",
            );
            ui.add_space(6.0);
            let options = &mut app.ui_state.planner_options;
            prop_row(ui, "Largest roughing end mill (mm)", |ui| {
                ui.add(
                    egui::DragValue::new(&mut options.roughing_tool_mm)
                        .range(0.8..=12.0)
                        .speed(0.1),
                );
            });
            prop_row(ui, "Largest finishing ball nose (mm)", |ui| {
                ui.add(
                    egui::DragValue::new(&mut options.finishing_tool_mm)
                        .range(0.8..=12.0)
                        .speed(0.1),
                );
            });
            prop_row(ui, "Steep wall angle (°)", |ui| {
                ui.add(
                    egui::DragValue::new(&mut options.steep_angle_deg)
                        .range(5.0..=85.0)
                        .speed(0.5),
                );
            });
            prop_row(ui, "Finish if stock left (mm)", |ui| {
                ui.add(
                    egui::DragValue::new(&mut options.residual_tolerance_mm)
                        .range(0.0..=2.0)
                        .speed(0.01),
                );
            });
            setup_changed |= ui
                .checkbox(
                    &mut app.project.setup.dynamic_stock_check,
                    "Dynamic stock collision check",
                )
                .on_hover_text(
                    "Replay every operation on the in-process stock: reports holder and rapid collisions, skips holes already cleared and plans finishing only where stock remains.",
                )
                .changed();
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "Holes are milled with a helical bore using an assumed end mill size; nothing is drilled. Re-planning replaces the operations of the previous plan.",
                )
                .size(11.0),
            );
            ui.add_space(6.0);
            if ui
                .add_enabled(!app.plan_pending, egui::Button::new("Plan operations"))
                .clicked()
            {
                run = true;
            }
        });
    if setup_changed {
        app.ui_state.project_dirty = true;
    }
    app.ui_state.show_auto_plan_dialog = open && !run;
    if run {
        app.start_auto_plan();
    }
}

fn invalidate_model_toolpaths(app: &mut TekkCamApp) {
    for operation in &mut app.project.operations {
        let planned_bores = operation.kind == crate::cam::operations::OperationKind::HelicalBore
            && !operation.bores.is_empty();
        if planned_bores
            || matches!(
                operation.kind,
                crate::cam::operations::OperationKind::ModelProfile2D
                    | crate::cam::operations::OperationKind::Roughing3D
                    | crate::cam::operations::OperationKind::FinishRaster3D
                    | crate::cam::operations::OperationKind::FinishContour3D
                    | crate::cam::operations::OperationKind::FinishWaterline3D
            )
        {
            operation.generated = false;
            operation.toolpath = Default::default();
        }
    }
    app.invalidate_gcode_and_simulation();
}

pub fn show_tool_database_dialog(ctx: &egui::Context, app: &mut TekkCamApp) {
    if !app.ui_state.show_tool_database {
        return;
    }

    let mut open = true;
    let selected_operation = app
        .ui_state
        .selected_operation
        .filter(|index| *index < app.project.operations.len());
    let mut choose_tool = None;
    let mut edit_tool = None;
    let mut delete_tool = None;
    let mut save_current = false;
    let mut view_tool = None;
    let built_in_tools = crate::cam::tool::built_in_tools();
    let custom_tools = app.project.tools.clone();

    egui::Window::new("Tool Database")
        .id(egui::Id::new("tool_database_dialog"))
        .default_width(440.0)
        .resizable(true)
        .open(&mut open)
        .show(ctx, |ui| {
            if let Some(index) = selected_operation {
                if ui.button("＋ Save selected operation's tool").clicked() {
                    save_current = true;
                }
            }
            ui.label(if selected_operation.is_some() {
                "Choose a tool to apply to the selected operation."
            } else {
                "Choose a tool to set it as the project default."
            });
            ui.separator();

            if app.project.tools.is_empty() && built_in_tools.is_empty() {
                ui.label("No tools saved yet. Use Create Tool to add one.");
            }

            egui::ScrollArea::vertical()
                .max_height(360.0)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    for (index, tool) in built_in_tools.iter().enumerate() {
                        ui.horizontal(|ui| {
                            show_tool_image(ui, ctx, app, &tool.name);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 10.0;
                                ui.strong(
                                    egui::RichText::new(&tool.name).color(tool_name_color(app)),
                                );
                                ui.label(tool_dimensions(&tool, app.ui_state.display_units));
                                ui.horizontal(|ui| {
                                    if ui
                                        .small_button("👁")
                                        .on_hover_text("View tool description")
                                        .clicked()
                                    {
                                        view_tool = Some(tool.clone());
                                    }
                                    if ui.button("Use").clicked() {
                                        choose_tool = Some((true, index));
                                    }
                                });
                            });
                        });
                        ui.add_space(6.0);
                    }
                    for (index, tool) in custom_tools.iter().enumerate() {
                        ui.horizontal(|ui| {
                            show_tool_image(ui, ctx, app, &tool.name);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 10.0;
                                ui.strong(
                                    egui::RichText::new(&tool.name).color(tool_name_color(app)),
                                );
                                ui.label(tool_dimensions(tool, app.ui_state.display_units));
                                ui.horizontal(|ui| {
                                    if ui
                                        .small_button("👁")
                                        .on_hover_text("View tool description")
                                        .clicked()
                                    {
                                        view_tool = Some(tool.clone());
                                    }
                                    if ui.button("Use").clicked() {
                                        choose_tool = Some((false, index));
                                    }
                                    if ui.small_button("Edit").clicked() {
                                        edit_tool = Some(index);
                                    }
                                    if ui.small_button("🗑").on_hover_text("Delete tool").clicked()
                                    {
                                        delete_tool = Some(index);
                                    }
                                });
                            });
                        });
                        ui.add_space(6.0);
                    }
                });
        });

    if !open {
        app.ui_state.show_tool_database = false;
    }
    if let Some(tool) = view_tool {
        app.ui_state.tool_details = Some(tool);
    }
    if save_current {
        if let Some(index) = selected_operation {
            app.project
                .tools
                .push(app.project.operations[index].tool.clone());
            app.ui_state.project_dirty = true;
            app.save_tool_database();
        }
    }
    if let Some(index) = choose_tool {
        let tool = if index.0 {
            built_in_tools[index.1].clone()
        } else {
            app.project.tools[index.1].clone()
        };
        if let Some(operation_index) = selected_operation {
            app.project.operations[operation_index].tool = tool;
        } else {
            app.project.default_tool = tool;
        }
        app.ui_state.project_dirty = true;
    }
    if let Some(index) = edit_tool {
        app.ui_state.tool_editor = Some(app.project.tools[index].clone());
        app.ui_state.tool_editor_index = Some(index);
        app.ui_state.tool_editor_error = None;
    }
    if let Some(index) = delete_tool {
        app.project.tools.remove(index);
        app.ui_state.project_dirty = true;
        app.save_tool_database();
    }
}

pub fn show_setup_dialog(ctx: &egui::Context, app: &mut TekkCamApp) {
    let units = app.ui_state.display_units;
    let mut changed = false;

    if app.ui_state.show_stock_dialog {
        let mut open = true;
        egui::Window::new("Stock Setup")
            .id(egui::Id::new("stock_setup_dialog"))
            .default_width(400.0)
            .resizable(true)
            .open(&mut open)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(600.0)
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
                        if let Some(model) = app.project.models.iter().find(|model| model.visible) {
                            let factor = units.factor();
                            ui.label(format!(
                                "Model bounds: {:.2} × {:.2} × {:.2} {}",
                                ((model.bounds[1][0] - model.bounds[0][0]) as f64) / factor,
                                ((model.bounds[1][1] - model.bounds[0][1]) as f64) / factor,
                                ((model.bounds[1][2] - model.bounds[0][2]) as f64) / factor,
                                units.label()
                            ));
                            ui.add_space(8.0);
                        }
                        let setup = &mut app.project.setup;
                        for (title, values) in [
                            ("Stock size", &mut setup.stock_size_mm),
                            ("Stock origin", &mut setup.stock_origin_mm),
                        ] {
                            ui.heading(title);
                            for (axis, value) in ["X", "Y", "Z"].into_iter().zip(values.iter_mut())
                            {
                                prop_row(ui, &format!("{} ({})", axis, units.label()), |ui| {
                                    changed |= unit_drag(ui, value, units, 0.1, -5000.0, 5000.0)
                                });
                            }
                        }
                        for (label, value) in [
                            ("Safe Z Height", &mut setup.safe_z_mm),
                            ("Clearance Height", &mut setup.clearance_mm),
                            ("Boundary Offset", &mut setup.boundary_margin_mm),
                            ("Maximum Stepdown", &mut setup.stepdown_mm),
                            ("Feed per Tooth", &mut setup.chip_load_mm),
                        ] {
                            prop_row(ui, &format!("{} ({})", label, units.label()), |ui| {
                                changed |= unit_drag(ui, value, units, 0.1, 0.0, 500.0)
                            });
                        }
                        prop_row(ui, "Stepover (% of tool diameter)", |ui| {
                            changed |= ui
                                .add(
                                    egui::DragValue::new(&mut setup.stepover_percent)
                                        .range(1.0..=95.0),
                                )
                                .changed()
                        });
                        prop_row(ui, "Ramp angle (°)", |ui| {
                            changed |= ui
                                .add(
                                    egui::DragValue::new(&mut setup.ramp_angle_deg)
                                        .range(0.0..=45.0),
                                )
                                .changed()
                        });
                        prop_row(ui, "Dynamic stock check", |ui| {
                            changed |= ui
                                .checkbox(&mut setup.dynamic_stock_check, "Collision check")
                                .on_hover_text(
                                    "Replay operations on the in-process stock to find holder and rapid collisions, and plan only the work still needed. Turn off for speed.",
                                )
                                .changed()
                        });
                        prop_row(ui, "Material", |ui| {
                            changed |= ui.text_edit_singleline(&mut setup.material).changed()
                        });
                    });
            });
        if !open {
            app.ui_state.show_stock_dialog = false;
        }
    }

    if app.ui_state.show_work_dialog {
        let mut open = true;
        egui::Window::new("Work Setup")
            .id(egui::Id::new("work_setup_dialog"))
            .default_width(360.0)
            .resizable(true)
            .open(&mut open)
            .show(ctx, |ui| {
                let setup = &mut app.project.setup;
                ui.heading("Work origin");
                for (axis, value) in ["X", "Y", "Z"]
                    .into_iter()
                    .zip(setup.work_origin_mm.iter_mut())
                {
                    prop_row(ui, &format!("{} ({})", axis, units.label()), |ui| {
                        changed |= unit_drag(ui, value, units, 0.1, -5000.0, 5000.0)
                    });
                }
                prop_row(ui, "Z orientation", |ui| {
                    egui::ComboBox::from_id_salt("setup_dialog_axis")
                        .selected_text(format!("{:?}", setup.axis))
                        .show_ui(ui, |ui| {
                            changed |= ui
                                .selectable_value(
                                    &mut setup.axis,
                                    crate::workspace::setup::WorkAxis::ZUp,
                                    "Z up",
                                )
                                .changed();
                            changed |= ui
                                .selectable_value(
                                    &mut setup.axis,
                                    crate::workspace::setup::WorkAxis::ZDown,
                                    "Z down",
                                )
                                .changed();
                        });
                });
                prop_row(ui, "Work offset", |ui| {
                    egui::ComboBox::from_id_salt("setup_dialog_offset")
                        .selected_text(setup.work_offset.code())
                        .show_ui(ui, |ui| {
                            for offset in [
                                crate::workspace::setup::WorkOffset::G54,
                                crate::workspace::setup::WorkOffset::G55,
                                crate::workspace::setup::WorkOffset::G56,
                                crate::workspace::setup::WorkOffset::G57,
                                crate::workspace::setup::WorkOffset::G58,
                                crate::workspace::setup::WorkOffset::G59,
                            ] {
                                changed |= ui
                                    .selectable_value(&mut setup.work_offset, offset, offset.code())
                                    .changed();
                            }
                        });
                });
            });
        if !open {
            app.ui_state.show_work_dialog = false;
        }
    }

    if changed {
        app.ui_state.project_dirty = true;
        app.scene_sig.clear();
    }
}

pub fn show_probing_dialog(ctx: &egui::Context, app: &mut TekkCamApp) {
    if !app.ui_state.show_probing_dialog {
        return;
    }
    use crate::workspace::probing::{
        grbl_probe_command, linuxcnc_surface_program, linuxcnc_touch_program, ProbeAxis, SurfaceMap,
    };
    let mut open = true;
    let mut error = None;
    let units = app.ui_state.display_units;
    egui::Window::new("CNC Probing")
        .id(egui::Id::new("probing_dialog"))
        .default_width(440.0)
        .resizable(true)
        .open(&mut open)
        .show(ctx, |ui| {
            let settings = &mut app.project.probing;
            ui.label("Touch-off");
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("probe_touch_axis")
                    .selected_text(format!("{:?} axis", settings.touch_axis))
                    .show_ui(ui, |ui| {
                        for axis in [ProbeAxis::X, ProbeAxis::Y, ProbeAxis::Z] {
                            ui.selectable_value(
                                &mut settings.touch_axis,
                                axis,
                                format!("{:?}", axis),
                            );
                        }
                    });
                ui.selectable_value(&mut settings.touch_negative, true, "− direction");
                ui.selectable_value(&mut settings.touch_negative, false, "+ direction");
            });
            ui.horizontal(|ui| {
                let factor = units.factor();
                let mut feed = settings.feed_mm_min / factor;
                if ui
                    .add(
                        egui::DragValue::new(&mut feed)
                            .speed(5.0 / factor)
                            .range(1.0 / factor..=500.0 / factor)
                            .prefix("Feed ")
                            .suffix(format!("{}/min", units.label())),
                    )
                    .changed()
                {
                    settings.feed_mm_min = feed * factor;
                }
                ui.label(format!("Travel ({})", units.label()));
                unit_drag(ui, &mut settings.max_travel_mm, units, 0.5, 0.1, 200.0);
            });
            ui.horizontal(|ui| {
                ui.label(format!("Retract ({})", units.label()));
                unit_drag(ui, &mut settings.retract_mm, units, 0.1, 0.1, 20.0);
                if settings.touch_axis == ProbeAxis::Z {
                    ui.label(format!("Plate ({})", units.label()));
                    unit_drag(
                        ui,
                        &mut settings.touch_plate_thickness_mm,
                        units,
                        0.1,
                        0.0,
                        100.0,
                    );
                } else {
                    ui.label(format!("Radius ({})", units.label()));
                    unit_drag(ui, &mut settings.probe_radius_mm, units, 0.1, 0.0, 100.0);
                }
            });
            let dialect = app.project.post_dialect;
            let machine_ready =
                app.machine.is_some() && app.ui_state.machine_status.state.starts_with("Idle");
            let grid_validation = validate_probe_grid(settings, &app.project.setup);
            ui.horizontal(|ui| {
                if dialect == crate::gcode::generator::Dialect::Grbl {
                    if ui
                        .add_enabled(machine_ready, egui::Button::new("Probe once…"))
                        .clicked()
                    {
                        app.ui_state.pending_probe_confirmation = Some(false);
                    }
                } else if ui.button("Export touch-off program…").clicked() {
                    match linuxcnc_touch_program(settings, app.project.setup.work_offset) {
                        Ok(program) => {
                            if let Some(path) = rfd::FileDialog::new()
                                .set_file_name("tekkcam-touch-off.ngc")
                                .save_file()
                            {
                                if let Err(e) = std::fs::write(path, program) {
                                    error = Some(e.to_string());
                                }
                            }
                        }
                        Err(e) => error = Some(e),
                    }
                }
                if ui.button("Export LinuxCNC grid program…").clicked() {
                    match grid_validation
                        .clone()
                        .and_then(|_| linuxcnc_surface_program(settings))
                    {
                        Ok(program) => {
                            if let Some(path) = rfd::FileDialog::new()
                                .set_file_name("tekkcam-surface-map.ngc")
                                .save_file()
                            {
                                if let Err(e) = std::fs::write(path, program) {
                                    error = Some(e.to_string());
                                }
                            }
                        }
                        Err(e) => error = Some(e),
                    }
                }
            });
            if dialect == crate::gcode::generator::Dialect::LinuxCnc
                && ui.button("Import LinuxCNC touch result…").clicked()
            {
                if let Some(path) = rfd::FileDialog::new().pick_file() {
                    match std::fs::read_to_string(path) {
                        Ok(content) => {
                            let result = content.lines().find_map(|line| {
                                let values: Vec<f64> = line
                                    .split(|c: char| {
                                        !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+')
                                    })
                                    .filter(|s| !s.is_empty())
                                    .filter_map(|s| s.parse().ok())
                                    .collect();
                                (values.len() >= 3)
                                    .then(|| ([values[0], values[1], values[2]], true))
                            });
                            if let Some(result) = result {
                                app.ui_state.last_probe_result = Some(result);
                            } else {
                                error = Some("No XYZ probe result was found in that file".into());
                            }
                        }
                        Err(e) => error = Some(e.to_string()),
                    }
                }
            }
            if let Some((position, success)) = app.ui_state.last_probe_result {
                ui.label(format!(
                    "Last probe: {} · X {:.3}, Y {:.3}, Z {:.3} {}",
                    if success { "triggered" } else { "failed" },
                    position[0] / units.factor(),
                    position[1] / units.factor(),
                    position[2] / units.factor(),
                    units.label()
                ));
                if success
                    && (dialect != crate::gcode::generator::Dialect::Grbl || machine_ready)
                    && ui
                        .button("Apply touch point to project work origin…")
                        .clicked()
                {
                    let p = app
                        .project
                        .setup
                        .work_offset
                        .code()
                        .chars()
                        .last()
                        .and_then(|c| c.to_digit(10))
                        .unwrap_or(4);
                    let axis = settings.touch_axis.letter();
                    let coordinate = if axis == 'Z' {
                        if settings.touch_negative {
                            settings.touch_plate_thickness_mm
                        } else {
                            -settings.touch_plate_thickness_mm
                        }
                    } else if settings.touch_negative {
                        settings.probe_radius_mm
                    } else {
                        -settings.probe_radius_mm
                    };
                    let command = format!("G10 L20 P{p} {axis}{coordinate:.3}");
                    if dialect == crate::gcode::generator::Dialect::Grbl {
                        if let Some(machine) = &app.machine {
                            machine.send_line(command);
                        }
                    }
                    match settings.touch_axis {
                        ProbeAxis::X => {
                            app.project.setup.work_origin_mm[0] = position[0] - coordinate
                        }
                        ProbeAxis::Y => {
                            app.project.setup.work_origin_mm[1] = position[1] - coordinate
                        }
                        ProbeAxis::Z => {
                            app.project.setup.work_origin_mm[2] = position[2] - coordinate
                        }
                    }
                    app.ui_state.project_dirty = true;
                    app.ui_state.console_log.push(
                        "Applied probe result to active controller work offset and project setup"
                            .into(),
                    );
                }
            }
            ui.separator();
            egui::CollapsingHeader::new("3D surface map")
                .default_open(false)
                .show(ui, |ui| {
                    for (label, values) in [
                        ("XY minimum", &mut settings.grid_min_xy),
                        ("XY maximum", &mut settings.grid_max_xy),
                    ] {
                        ui.horizontal(|ui| {
                            ui.label(label);
                            for (axis, value) in ["X", "Y"].into_iter().zip(values.iter_mut()) {
                                let factor = units.factor();
                                let mut displayed = *value / factor;
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut displayed)
                                            .speed(0.5 / factor)
                                            .prefix(format!("{axis} "))
                                            .suffix(format!(" {}", units.label())),
                                    )
                                    .changed()
                                {
                                    *value = displayed * factor;
                                }
                            }
                        });
                    }
                    ui.label(format!("Grid spacing ({})", units.label()));
                    unit_drag(ui, &mut settings.grid_spacing_mm, units, 0.5, 0.1, 500.0);
                    ui.add(
                        egui::DragValue::new(&mut settings.max_grid_points)
                            .range(4..=10000)
                            .prefix("Point limit: "),
                    );
                    match settings.grid_points() {
                        Ok(points) => {
                            ui.label(format!("{} points", points.len()));
                        }
                        Err(e) => {
                            error = Some(e);
                        }
                    }
                    if dialect == crate::gcode::generator::Dialect::Grbl {
                        if ui
                            .add_enabled(
                                machine_ready && grid_validation.is_ok(),
                                egui::Button::new("Run GRBL surface map…"),
                            )
                            .clicked()
                        {
                            app.ui_state.pending_probe_confirmation = Some(true);
                        }
                    }
                    if ui.button("Export LinuxCNC surface-map program…").clicked() {
                        match grid_validation
                            .clone()
                            .and_then(|_| linuxcnc_surface_program(settings))
                        {
                            Ok(program) => {
                                if let Some(path) = rfd::FileDialog::new()
                                    .set_file_name("tekkcam-surface-map.ngc")
                                    .save_file()
                                {
                                    if let Err(e) = std::fs::write(path, program) {
                                        error = Some(e.to_string());
                                    }
                                }
                            }
                            Err(e) => error = Some(e),
                        }
                    }
                    if ui.button("Import LinuxCNC probe-results.txt…").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_file() {
                            match std::fs::read_to_string(path) {
                                Ok(content) => {
                                    let points: Vec<[f64; 3]> = content
                                        .lines()
                                        .filter_map(|line| {
                                            let values: Vec<f64> = line
                                                .split(|c: char| {
                                                    !(c.is_ascii_digit()
                                                        || c == '.'
                                                        || c == '-'
                                                        || c == '+')
                                                })
                                                .filter(|s| !s.is_empty())
                                                .filter_map(|s| s.parse().ok())
                                                .collect();
                                            (values.len() >= 3)
                                                .then(|| [values[0], values[1], values[2]])
                                        })
                                        .collect();
                                    match settings.grid_points() {
                                        Ok(grid)
                                            if grid.len() == points.len()
                                                && points
                                                    .iter()
                                                    .flatten()
                                                    .all(|v| v.is_finite()) =>
                                        {
                                            app.project.surface_map = Some(SurfaceMap {
                                                settings: settings.clone(),
                                                points,
                                            });
                                            app.ui_state.project_dirty = true;
                                        }
                                        Ok(grid) => {
                                            error = Some(format!(
                                                "Imported {} valid points; expected {} grid points",
                                                points.len(),
                                                grid.len()
                                            ))
                                        }
                                        Err(e) => error = Some(e),
                                    }
                                }
                                Err(e) => error = Some(e.to_string()),
                            }
                        }
                    }
                    if let Some(map) = &app.project.surface_map {
                        ui.label(format!("Saved map: {} points", map.points.len()));
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(300.0, 180.0), egui::Sense::hover());
                        ui.painter()
                            .rect_filled(rect, 4.0, egui::Color32::from_gray(28));
                        ui.painter().rect_stroke(
                            rect,
                            4.0,
                            egui::Stroke::new(1.0_f32, egui::Color32::GRAY),
                            egui::StrokeKind::Inside,
                        );
                        if let (Some(min_z), Some(max_z)) = (
                            map.points.iter().map(|p| p[2]).reduce(f64::min),
                            map.points.iter().map(|p| p[2]).reduce(f64::max),
                        ) {
                            let dx = (map.settings.grid_max_xy[0] - map.settings.grid_min_xy[0])
                                .max(f64::EPSILON);
                            let dy = (map.settings.grid_max_xy[1] - map.settings.grid_min_xy[1])
                                .max(f64::EPSILON);
                            for point in &map.points {
                                let px = rect.left()
                                    + ((point[0] - map.settings.grid_min_xy[0]) / dx) as f32
                                        * rect.width();
                                let py = rect.bottom()
                                    - ((point[1] - map.settings.grid_min_xy[1]) / dy) as f32
                                        * rect.height();
                                let t = if max_z > min_z {
                                    ((point[2] - min_z) / (max_z - min_z)) as f32
                                } else {
                                    0.5
                                };
                                let color = egui::Color32::from_rgb(
                                    (50.0 + 190.0 * t) as u8,
                                    (180.0 - 120.0 * t) as u8,
                                    (240.0 - 190.0 * t) as u8,
                                );
                                ui.painter().circle_filled(egui::pos2(px, py), 4.0, color);
                            }
                        }
                        if ui
                            .checkbox(
                                &mut app.project.surface_compensation_enabled,
                                "Compensate generated toolpaths using this map",
                            )
                            .changed()
                        {
                            app.ui_state.project_dirty = true;
                        }
                        if ui.button("Clear saved surface map").clicked() {
                            app.project.surface_map = None;
                            app.project.surface_compensation_enabled = false;
                            app.ui_state.project_dirty = true;
                        }
                    }
                });
            if let Some(error) = &error {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
        });
    if !open {
        app.ui_state.show_probing_dialog = false;
    }
    if let Some(surface_map) = app.ui_state.pending_probe_confirmation {
        let mut confirm_open = true;
        egui::Window::new("Confirm probe motion")
            .collapsible(false)
            .resizable(false)
            .open(&mut confirm_open)
            .show(ctx, |ui| {
                ui.label("Probe motion will move the machine.");
                ui.label("Check probe wiring, starting position, clearance, and feed.");
                ui.horizontal(|ui| {
                    if ui
                        .button(if surface_map {
                            "Run surface map"
                        } else {
                            "Start probe"
                        })
                        .clicked()
                    {
                        let settings = &app.project.probing;
                        if let Some(machine) = &app.machine {
                            if surface_map {
                                match settings.grid_points() {
                                    Ok(points) if !points.is_empty() => {
                                        let safe_z = app.project.setup.stock_origin_mm[2]
                                            + app.project.setup.stock_size_mm[2]
                                            + app.project.setup.clearance_mm;
                                        let mut lines = vec![format!("G90 G0 Z{safe_z:.3}")];
                                        for [x, y] in points {
                                            lines.push(format!("G90 G0 X{x:.3} Y{y:.3}"));
                                            lines.push(format!(
                                                "G91 G38.2 Z-{:.3} F{:.3}",
                                                settings.max_travel_mm, settings.feed_mm_min
                                            ));
                                            lines.push("G90".into());
                                            lines.push(format!("G0 Z{safe_z:.3}"));
                                        }
                                        app.ui_state.probe_capture_points.clear();
                                        app.ui_state.active_probe_settings = Some(settings.clone());
                                        machine.stream(&lines.join("\n"));
                                        app.ui_state.status_message =
                                            "Running GRBL surface probe map".into();
                                    }
                                    Ok(_) => {}
                                    Err(e) => error = Some(e),
                                }
                            } else {
                                let direction = if settings.touch_negative { -1.0 } else { 1.0 };
                                match settings.validate().and_then(|_| {
                                    grbl_probe_command(
                                        settings.touch_axis,
                                        direction * settings.max_travel_mm,
                                        settings.feed_mm_min,
                                    )
                                }) {
                                    Ok(command) => {
                                        machine.send_line(command);
                                        app.ui_state.last_probe_result = None;
                                        app.ui_state.status_message =
                                            "Probing; machine motion active".into();
                                    }
                                    Err(e) => error = Some(e),
                                }
                            }
                        }
                        app.ui_state.pending_probe_confirmation = None;
                    }
                    if ui.button("Cancel").clicked() {
                        app.ui_state.pending_probe_confirmation = None;
                    }
                });
            });
        if !confirm_open {
            app.ui_state.pending_probe_confirmation = None;
        }
    }
}

fn validate_probe_grid(
    settings: &crate::workspace::probing::ProbeSettings,
    setup: &crate::workspace::setup::MachiningSetup,
) -> Result<(), String> {
    settings.validate()?;
    for [x, y] in settings.grid_points()? {
        if x < setup.stock_origin_mm[0]
            || x > setup.stock_origin_mm[0] + setup.stock_size_mm[0]
            || y < setup.stock_origin_mm[1]
            || y > setup.stock_origin_mm[1] + setup.stock_size_mm[1]
        {
            return Err(format!(
                "probe point ({x:.2}, {y:.2}) is outside the configured stock"
            ));
        }
    }
    let safe_z = setup.stock_origin_mm[2] + setup.stock_size_mm[2] + setup.clearance_mm;
    if let Some(limits) = &setup.machine_limits {
        if safe_z > limits.max_mm[2] || safe_z - settings.max_travel_mm < limits.min_mm[2] {
            return Err("probe Z travel exceeds configured machine limits".into());
        }
    }
    Ok(())
}

fn tool_dimensions(tool: &crate::cam::tool::Tool, units: crate::app::DisplayUnits) -> String {
    let factor = units.factor();
    format!(
        "Ø {:.2} {} · flute {:.1} {}",
        tool.diameter_mm / factor,
        units.label(),
        tool.flute_length_mm / factor,
        units.label()
    )
}

fn tool_name_color(app: &TekkCamApp) -> egui::Color32 {
    if app.ui_state.theme == crate::app::AppTheme::Light {
        egui::Color32::from_rgb(30, 32, 40)
    } else {
        egui::Color32::from_rgb(240, 240, 245)
    }
}

fn show_tool_image(ui: &mut egui::Ui, ctx: &egui::Context, app: &mut TekkCamApp, name: &str) {
    let filename = if name.starts_with("4 mm Carbide Endmill") {
        "4-mm-carbide-endmill.png".to_owned()
    } else if name.starts_with("2 mm Carbide Endmill") {
        "2-mm-carbide-endmill.png".to_owned()
    } else if name.starts_with("6 mm Carbide Endmill") {
        "6-mm-carbide-endmill.png".to_owned()
    } else if name.starts_with("4 mm Single Flute Endmill") {
        "4-mm-single-flute-endmill.png".to_owned()
    } else {
        let slug: String = name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let slug = slug
            .split('-')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        format!("{slug}.png")
    };
    let path = std::path::Path::new("assets/tools").join(filename);
    let key = path.to_string_lossy().to_string();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(96.0, 96.0), egui::Sense::hover());
    if ui.is_rect_visible(rect) && !app.ui_state.tool_image_cache.contains_key(&key) {
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(image) = image::load_from_memory(&bytes) {
                let image = image.to_rgba8();
                let size = [image.width() as usize, image.height() as usize];
                let color = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
                let texture = ctx.load_texture(&key, color, egui::TextureOptions::LINEAR);
                app.ui_state
                    .tool_image_cache
                    .insert(key.clone(), Some(texture));
            } else {
                app.ui_state.tool_image_cache.insert(key.clone(), None);
            }
        } else {
            app.ui_state.tool_image_cache.insert(key.clone(), None);
        }
    }
    if let Some(Some(texture)) = app.ui_state.tool_image_cache.get(&key) {
        ui.put(
            rect,
            egui::Image::new(texture).fit_to_exact_size(rect.size()),
        );
    }
}

pub fn show_tool_details_dialog(ctx: &egui::Context, app: &mut TekkCamApp) {
    let Some(tool) = app.ui_state.tool_details.clone() else {
        return;
    };
    let units = app.ui_state.display_units;
    let factor = units.factor();
    let mut open = true;
    egui::Window::new("Tool description")
        .id(egui::Id::new("tool_details_dialog"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.strong(&tool.name);
            ui.separator();
            ui.label(format!(
                "{} {} diameter; {:.1} {} flute length.",
                tool.diameter_mm / factor,
                units.label(),
                tool.flute_length_mm / factor,
                units.label()
            ));
            if !tool.description.is_empty() {
                ui.label(&tool.description);
            } else {
                ui.label(format!(
                    "{:?} cutter. Additional product description is not available.",
                    tool.shape
                ));
            }
            ui.label(format!(
                "Default CAM settings: feed {:.0} {}/min, plunge {:.0} {}/min, spindle {:.0} RPM.",
                tool.feed_rate_mm_min / factor,
                units.label(),
                tool.plunge_rate_mm_min / factor,
                units.label(),
                tool.spindle_rpm
            ));
        });
    if !open {
        app.ui_state.tool_details = None;
    }
}

pub fn show_tool_editor(ctx: &egui::Context, app: &mut TekkCamApp) {
    let Some(mut tool) = app.ui_state.tool_editor.take() else {
        return;
    };
    let mut open = true;
    let mut save = false;
    let mut cancel = false;
    let units = app.ui_state.display_units;
    egui::Window::new(if app.ui_state.tool_editor_index.is_some() {
        "Edit cutting tool"
    } else {
        "Create cutting tool"
    })
    .id(egui::Id::new("tool_editor"))
    .collapsible(false)
    .resizable(false)
    .open(&mut open)
    .show(ctx, |ui| {
        ui.set_min_width(340.0);
        egui::Grid::new("tool_editor_fields")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut tool.name);
                ui.end_row();
                ui.label("Shape");
                egui::ComboBox::from_id_salt("tool_editor_shape")
                    .selected_text(format!("{:?}", tool.shape))
                    .show_ui(ui, |ui| {
                        for shape in [
                            crate::cam::tool::ToolShape::Flat,
                            crate::cam::tool::ToolShape::BallNose,
                            crate::cam::tool::ToolShape::BullNose,
                        ] {
                            ui.selectable_value(&mut tool.shape, shape, format!("{:?}", shape));
                        }
                    });
                ui.end_row();
                for (label, value, rate) in [
                    ("Diameter", &mut tool.diameter_mm, false),
                    ("Flute length", &mut tool.flute_length_mm, false),
                    ("Stickout", &mut tool.stickout_mm, false),
                    ("Corner radius", &mut tool.corner_radius_mm, false),
                    ("Holder clearance", &mut tool.holder_clearance_mm, false),
                    ("Cutting Feedrate", &mut tool.feed_rate_mm_min, true),
                    ("Plunge Feedrate", &mut tool.plunge_rate_mm_min, true),
                    ("Spindle Speed (RPM)", &mut tool.spindle_rpm, false),
                ] {
                    let is_spindle = label.starts_with("Spindle");
                    ui.label(if is_spindle {
                        label.to_owned()
                    } else if rate {
                        format!("{} ({}/min)", label, units.label())
                    } else {
                        format!("{} ({})", label, units.label())
                    });
                    if is_spindle {
                        ui.add(egui::DragValue::new(value).speed(0.1).min_decimals(1));
                    } else {
                        unit_drag(ui, value, units, 0.1, 0.0, 10000.0);
                    }
                    ui.end_row();
                }
            });
        if let Some(error) = &app.ui_state.tool_editor_error {
            ui.add_space(6.0);
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            save = ui.button("Save to tool database").clicked();
            cancel = ui.button("Cancel").clicked();
        });
    });

    if cancel || !open {
        app.ui_state.tool_editor_index = None;
        app.ui_state.tool_editor_error = None;
        return;
    }
    if save {
        let result = if tool.name.trim().is_empty() {
            Err("Tool name cannot be empty".to_owned())
        } else {
            tool.validate()
        };
        match result {
            Ok(()) => {
                if let Some(index) = app.ui_state.tool_editor_index {
                    if let Some(existing) = app.project.tools.get_mut(index) {
                        *existing = tool;
                    }
                } else {
                    app.project.tools.push(tool);
                }
                app.ui_state.project_dirty = true;
                app.save_tool_database();
                app.ui_state.tool_editor_index = None;
                app.ui_state.tool_editor_error = None;
                return;
            }
            Err(error) => app.ui_state.tool_editor_error = Some(error),
        }
    }
    app.ui_state.tool_editor = Some(tool);
}

fn unit_drag(
    ui: &mut egui::Ui,
    millimeters: &mut f64,
    units: crate::app::DisplayUnits,
    speed_mm: f64,
    min_mm: f64,
    max_mm: f64,
) -> bool {
    let factor = units.factor();
    let mut displayed = *millimeters / factor;
    let changed = ui
        .add(
            egui::DragValue::new(&mut displayed)
                .speed(speed_mm / factor)
                .range(min_mm / factor..=max_mm / factor),
        )
        .changed();
    if changed {
        *millimeters = displayed * factor;
    }
    changed
}

fn prop_row(ui: &mut egui::Ui, label: &str, add_content: impl FnOnce(&mut egui::Ui)) {
    let label_color = if ui.visuals().dark_mode {
        egui::Color32::from_rgb(160, 160, 180)
    } else {
        egui::Color32::from_rgb(72, 75, 106)
    };
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(12.0).color(label_color));
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            add_content,
        );
    });
    ui.add_space(2.0);
}
