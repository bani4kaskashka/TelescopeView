#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod app;
mod canvas;
mod files;
mod instance;
mod keys;
mod loader;
mod system;
mod theme;
mod thumbs;
mod ui;
mod video;
mod winimg;

use eframe::egui;
use std::path::PathBuf;

fn main() -> eframe::Result {
    // "Open with", a double-clicked file, dragging onto the exe, or `TelescopeView <file|folder>`.
    let initial = std::env::args_os().nth(1).map(PathBuf::from);
    // `--register` / `--unregister` from the installer: done, without a window.
    if initial.as_ref().is_some_and(|a| system::command_line(a.as_os_str())) {
        return Ok(());
    }
    // Already open (and set to take files): shown there instead of in a second window.
    if let Some(p) = &initial
        && instance::hand_off(p)
    {
        return Ok(());
    }
    migrate_settings();
    // Show up in "Open with" and Default apps. Done off the UI thread; it's a few registry writes.
    std::thread::spawn(system::register_if_needed);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(app::APP_NAME)
            .with_app_id(app::APP_NAME)
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([480.0, 320.0])
            .with_drag_and_drop(true)
            .with_icon(std::sync::Arc::new(theme::app_icon())),
        renderer: renderer(),
        ..Default::default()
    };
    eframe::run_native(app::APP_NAME, options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, initial)))))
}

/// Settings and reading history from before the rename.
fn migrate_settings() {
    let (Some(new), Some(old)) = (eframe::storage_dir(app::APP_NAME), eframe::storage_dir("Image Viewer")) else {
        return;
    };
    let (new_file, old_file) = (new.join("app.ron"), old.join("app.ron"));
    if !new_file.exists() && old_file.exists() {
        let _ = std::fs::create_dir_all(&new).and_then(|_| std::fs::copy(&old_file, &new_file));
    }
}

/// OpenGL by default: on AMD drivers wgpu (DX12 and Vulkan alike) keeps a driver thread busy
/// on a full CPU core even while the window is idle. `IV_RENDERER=wgpu` switches back.
fn renderer() -> eframe::Renderer {
    match std::env::var("IV_RENDERER").as_deref() {
        Ok("wgpu") => eframe::Renderer::Wgpu,
        _ => eframe::Renderer::Glow,
    }
}
