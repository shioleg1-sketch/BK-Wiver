#[cfg(windows)]
fn main() {
    let mut res = winres::WindowsResource::new();
    res.set_icon("../assets/app-icon.ico");
    if let Err(error) = res.compile() {
        panic!("failed to compile consol Windows resources: {error}");
    }
}

#[cfg(not(windows))]
fn main() {}
