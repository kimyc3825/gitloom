// Release builds are GUI apps on Windows: no console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod fonts;
mod forge;
mod git;
mod jobs;
mod search;
mod ui;
mod util;

use std::path::PathBuf;

use eframe::egui;

fn main() -> eframe::Result {
    // `gitloom [path]`; without a path, use the current directory if it is a repository.
    let initial_repo = std::env::args_os().nth(1).map(PathBuf::from).or_else(|| {
        std::env::current_dir()
            .ok()
            .filter(|dir| git::repo::discover(dir).is_ok())
    });

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("gitloom")
            .with_app_id("gitloom")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 600.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };

    eframe::run_native(
        "gitloom",
        options,
        Box::new(move |cc| {
            fonts::install(&cc.egui_ctx);
            Ok(Box::new(app::GitloomApp::new(
                cc.egui_ctx.clone(),
                initial_repo,
            )))
        }),
    )
}
