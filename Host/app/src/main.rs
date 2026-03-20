#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod app;
mod capture;
mod logging;
mod media;
mod signal;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::run()
}
