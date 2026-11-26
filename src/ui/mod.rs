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

    egui::Frame::new()
        .fill(track_bg)
        .stroke(egui::Stroke::new(1.0_f32, track_border))
        .corner_radius(egui::CornerRadius::same(18))
        .inner_margin(egui::Margin::same(3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for &(value, icon, tooltip) in options {
                    let active = *current == value;
                    let button = egui::Button::new(
                        egui::RichText::new(icon.to_string())
                            .font(egui::FontId::new(
                                16.0,
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
                    .corner_radius(egui::CornerRadius::same(14));
                    if ui
                        .add_sized([36.0, 28.0], button)
                        .on_hover_text(tooltip)
                        .clicked()
                        && !active
                    {
                        *current = value;
                        changed = true;
                    }
                }
            });
        });
    changed
}
