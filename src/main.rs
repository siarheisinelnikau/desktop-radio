#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod canvas;
mod hls;
mod http;
mod media;
mod model;
mod strings;
mod ui;
mod updates;

fn main() -> std::process::ExitCode {
    if let Err(error) = app::run() {
        eprintln!("{error:#}");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}
