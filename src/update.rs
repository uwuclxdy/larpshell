use std::sync::OnceLock;

use colored::Colorize;
use serde::Deserialize;
use tokio::task::JoinHandle;

use crate::common::CTP_PRIMARY;
use crate::confirmation::style_message_markup;

static UPDATE_RESULT: OnceLock<bool> = OnceLock::new();

#[derive(Deserialize)]
struct CrateInfo {
    #[serde(rename = "crate")]
    krate: CrateData,
}

#[derive(Deserialize)]
struct CrateData {
    newest_version: String,
}

enum InstallMethod {
    Cargo,
    Aur,
    Unknown,
}

fn detect_install_method() -> InstallMethod {
    let Ok(exe) = std::env::current_exe() else {
        return InstallMethod::Unknown;
    };
    let path = exe.to_string_lossy();

    if path.starts_with("/usr") {
        InstallMethod::Aur
    } else if path.contains(".cargo/bin") {
        InstallMethod::Cargo
    } else {
        InstallMethod::Unknown
    }
}

fn update_instruction() -> &'static str {
    match detect_install_method() {
        InstallMethod::Cargo => ", run `cargo install larpshell`",
        InstallMethod::Aur => ", run `yay -S larpshell` or `yay -S larpshell-git`",
        InstallMethod::Unknown => "",
    }
}

pub async fn is_update_available() -> bool {
    let Ok(client) = reqwest::Client::builder()
        .user_agent(concat!("larpshell/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(5))
        .build()
    else {
        return false;
    };
    let Ok(response) = client
        .get("https://crates.io/api/v1/crates/larpshell")
        .send()
        .await
    else {
        return false;
    };
    let Ok(info) = response.json::<CrateInfo>().await else {
        return false;
    };
    let available = info.krate.newest_version != env!("CARGO_PKG_VERSION");
    UPDATE_RESULT.set(available).ok();
    available
}

/// Prints the update notice if the background check has already completed and
/// found a newer version. Safe to call from any context — no-ops if the result
/// is not yet available.
pub fn print_if_resolved() {
    if *UPDATE_RESULT.get().unwrap_or(&false) {
        print_notice();
    }
}

fn print_notice() {
    let mut msg = "update available".to_string();
    msg.push_str(update_instruction());
    eprintln!(
        "{}",
        style_message_markup(&msg).custom_color(CTP_PRIMARY).bold()
    );
}

pub async fn print_if_available(task: JoinHandle<bool>) {
    if matches!(task.await, Ok(true)) {
        print_notice();
    }
}
