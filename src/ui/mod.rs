pub mod console;
pub mod machine;
pub mod orientation_preview;
pub mod palette;
pub mod properties;
pub mod settings;
pub mod sidebar;
pub mod toolbar;
pub mod viewport;

use eframe::egui;

/// Renders a pill-shaped segmented icon toggle matching the compact rounded
/// switcher style. Returns `true` when the selected value changed.
pub fn icon_pill_toggle<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    dark: bool,
    current: &mut T,
    options: &[(T, char, &str)],
) -> bool {
    icon_pill_toggle_sized(ui, dark, current, options, [36.0, 28.0], 16.0, 3)
}

/// Renders a compact pill-shaped segmented icon toggle for dense toolbars.
pub fn icon_pill_toggle_compact<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    dark: bool,
    current: &mut T,
    options: &[(T, char, &str)],
) -> bool {
    icon_pill_toggle_sized(ui, dark, current, options, [26.0, 25.0], 13.0, 2)
}

fn icon_pill_toggle_sized<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    dark: bool,
    current: &mut T,
    options: &[(T, char, &str)],
    button_size: [f32; 2],
    icon_size: f32,
    margin: i8,
) -> bool {
    let mut changed = false;
    let track_bg = if dark {
        egui::Color32::from_rgb(20, 21, 25)
    } else {
        egui::Color32::from_rgb(215, 217, 229)
    };
    let track_border = if dark {
        egui::Color32::from_rgb(52, 54, 64)
    } else {
        egui::Color32::from_rgb(185, 188, 204)
    };
    let active_bg = if dark {
        egui::Color32::from_rgb(42, 43, 52)
    } else {
        egui::Color32::WHITE
    };
    let active_border = if dark {
        egui::Color32::from_rgb(76, 78, 92)
    } else {
        egui::Color32::from_rgb(176, 180, 198)
    };
    let active_fg = if dark {
        egui::Color32::WHITE
    } else {
        egui::Color32::from_rgb(45, 47, 64)
    };
    let inactive_fg = if dark {
        egui::Color32::from_rgb(150, 154, 170)
    } else {
        egui::Color32::from_rgb(115, 118, 136)
    };

    let outer_radius = ((button_size[1] + f32::from(margin) * 2.0) * 0.5).ceil() as u8;
    let inner_radius = (button_size[1] * 0.5).ceil() as u8;
    let inner_width =
        options.len() as f32 * button_size[0] + options.len().saturating_sub(1) as f32 * 2.0;
    egui::Frame::new()
        .fill(track_bg)
        .stroke(egui::Stroke::new(1.0_f32, track_border))
        .corner_radius(egui::CornerRadius::same(outer_radius))
        .inner_margin(egui::Margin::same(margin))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.spacing_mut().interact_size.y = button_size[1];
            ui.spacing_mut().button_padding = egui::Vec2::ZERO;
            ui.allocate_ui_with_layout(
                egui::vec2(inner_width, button_size[1]),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    for &(value, icon, tooltip) in options {
                        let active = *current == value;
                        let button = egui::Button::new(
                            egui::RichText::new(icon.to_string())
                                .font(egui::FontId::new(
                                    icon_size,
                                    egui::FontFamily::Name("sidebar_icons".into()),
                                ))
                                .color(if active { active_fg } else { inactive_fg }),
                        )
                        .fill(if active {
                            active_bg
                        } else {
                            egui::Color32::TRANSPARENT
                        })
                        .stroke(if active {
                            egui::Stroke::new(1.0_f32, active_border)
                        } else {
                            egui::Stroke::NONE
                        })
                        .corner_radius(egui::CornerRadius::same(inner_radius));
                        if ui
                            .add_sized(button_size, button)
                            .on_hover_text(tooltip)
                            .clicked()
                            && !active
                        {
                            *current = value;
                            changed = true;
                        }
                    }
                },
            );
        });
    changed
}
