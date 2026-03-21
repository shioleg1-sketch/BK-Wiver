use std::{
    env, fs,
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

const CONSOLE_LOG_FILE: &str = "console-runtime.log";

pub fn append_log(level: &str, source: &str, message: impl AsRef<str>) {
    let _ = ensure_state_dir();
    let line = format!(
        "[{}] [{}] [{}] {}\n",
        now_ms(),
        level,
        source,
        message.as_ref()
    );

    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(console_log_path())
    {
        let _ = file.write_all(line.as_bytes());
    }
}

pub fn export_diagnostic_report(status_line: &str) -> Result<PathBuf, String> {
    ensure_state_dir()?;

    let export_dir = desktop_dir().unwrap_or_else(app_state_dir);
    fs::create_dir_all(&export_dir).map_err(|error| error.to_string())?;

    let export_path = export_dir.join(format!("BK-Console-log-{}.txt", now_ms()));
    let mut body = String::new();
    body.push_str("BK-Console diagnostic report\n");
    body.push_str("===========================\n\n");
    body.push_str(&format!("generated_at_ms={}\n", now_ms()));
    body.push_str(&format!("state_dir={}\n", app_state_dir().display()));
    body.push_str(&format!("status_line={}\n\n", status_line));

    body.push_str("--- console-runtime.log ---\n");
    match fs::read_to_string(console_log_path()) {
        Ok(contents) => body.push_str(&contents),
        Err(error) => body.push_str(&format!("unavailable: {}\n", error)),
    }
    body.push('\n');

    fs::write(&export_path, body).map_err(|error| error.to_string())?;
    Ok(export_path)
}

pub fn write_state_bytes(file_name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    ensure_state_dir()?;
    let path = app_state_dir().join(file_name);
    fs::write(&path, bytes).map_err(|error| error.to_string())?;
    Ok(path)
}

fn console_log_path() -> PathBuf {
    app_state_dir().join(CONSOLE_LOG_FILE)
}

fn ensure_state_dir() -> Result<(), String> {
    fs::create_dir_all(app_state_dir()).map_err(|error| error.to_string())
}

fn app_state_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        return home
            .join("Library")
            .join("Application Support")
            .join("BK-Wiver")
            .join("state");
    }

    #[cfg(not(target_os = "macos"))]
    {
        let local_app_data = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        local_app_data.join("BK-Wiver").join("state")
    }
}

fn desktop_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = env::var_os("HOME").map(PathBuf::from)?;
        return Some(home.join("Desktop"));
    }

    #[cfg(not(target_os = "macos"))]
    {
        let user_profile = env::var_os("USERPROFILE").map(PathBuf::from)?;
        Some(user_profile.join("Desktop"))
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
