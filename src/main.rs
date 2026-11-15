// Scaffold phase: allow dead_code and unused for placeholder modules
#![allow(dead_code, unused_variables, unused_imports)]

fn main() -> anyhow::Result<()> {
    // Hyprland's native Wayland file-drop path is unreliable for this app.
    // Match Muzik's workaround so eframe falls back to the X11/XWayland path.
    if std::env::var("XDG_SESSION_TYPE").is_ok_and(|session| session == "wayland") {
        unsafe {
            std::env::remove_var("WAYLAND_DISPLAY");
        }
        // Keep the XWayland UI at the preferred fractional scale for this setup.
        std::env::set_var("WINIT_X11_SCALE_FACTOR", "1.25");
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(
        // Show our own crate at INFO, suppress noisy wgpu/zbus internals
        "warn,tekkcam=info,eframe=info,egui_wgpu=info,wgpu=warn",
    ))
    .init();

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("TekkCAM")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([900.0, 600.0])
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
                    .unwrap_or_default(),
            ),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    eframe::run_native(
        "TekkCAM",
        native_options,
        Box::new(|cc| Ok(Box::new(tekkcam::app::TekkCamApp::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {e}"))
}
