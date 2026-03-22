use std::{
    env,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=BK_WIVER_COMMIT");
    println!("cargo:rerun-if-env-changed=BK_WIVER_BUILD_ID");
    emit_git_rerun_triggers();

    let commit = env::var("BK_WIVER_COMMIT")
        .ok()
        .or_else(|| git_output(&["rev-parse", "--short=10", "HEAD"]))
        .unwrap_or_else(|| "dev".to_owned());
    let build_id = env::var("BK_WIVER_BUILD_ID").unwrap_or_else(|_| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_secs().to_string())
            .unwrap_or_else(|_| "0".to_owned())
    });

    println!("cargo:rustc-env=BK_WIVER_COMMIT={commit}");
    println!("cargo:rustc-env=BK_WIVER_BUILD_ID={build_id}");
}

fn emit_git_rerun_triggers() {
    let Some(git_dir) = git_output(&["rev-parse", "--git-dir"]) else {
        return;
    };

    let git_dir = PathBuf::from(git_dir);
    println!("cargo:rerun-if-changed={}", git_dir.join("HEAD").display());

    if let Some(head_ref) = git_output(&["symbolic-ref", "-q", "HEAD"]) {
        println!(
            "cargo:rerun-if-changed={}",
            git_dir.join(head_ref).display()
        );
    }
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8(output.stdout).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}
