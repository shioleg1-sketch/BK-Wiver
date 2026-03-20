#[cfg(target_os = "macos")]
#[path = "../../../Consol/app/src/api.rs"]
mod api;

#[cfg(target_os = "macos")]
#[path = "../../../Consol/app/src/app.rs"]
mod app;

#[cfg(target_os = "macos")]
#[path = "../../../Consol/app/src/signal.rs"]
mod signal;

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::run()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("bk-wiver-console-macos is intended to run on macOS only.");
}
