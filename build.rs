use chrono::{DateTime, Utc};
use std::{env, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    println!(
        "cargo:rustc-env=PHP_DOWNLOADER_GIT_SHA={}",
        git_sha().unwrap_or_else(|| "unknown".into())
    );
    println!(
        "cargo:rustc-env=PHP_DOWNLOADER_BUILD_DATE={}",
        build_date().unwrap_or_else(|| "unknown".into())
    );
}

fn git_sha() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()?;

    output.status.success().then_some(())?;

    String::from_utf8(output.stdout)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn build_date() -> Option<String> {
    if let Ok(epoch) = env::var("SOURCE_DATE_EPOCH") {
        if let Ok(secs) = epoch.parse::<i64>() {
            if let Some(dt) = DateTime::<Utc>::from_timestamp(secs, 0) {
                return Some(dt.to_rfc3339());
            }
        }
    }

    Some(Utc::now().to_rfc3339())
}
