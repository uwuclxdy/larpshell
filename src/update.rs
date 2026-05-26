use colored::Colorize;
use serde::Deserialize;
use tokio::task::JoinHandle;

use crate::common::CTP_PRIMARY;
use crate::confirmation::style_message_markup;

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

    if path.contains(".cargo/bin") {
        InstallMethod::Cargo
    } else if path.starts_with("/usr") {
        let is_arch = std::path::Path::new("/etc/arch-release").exists();
        if is_arch {
            InstallMethod::Aur
        } else {
            InstallMethod::Unknown
        }
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
    remote_version_is_newer(&info.krate.newest_version, env!("CARGO_PKG_VERSION"))
}

fn remote_version_is_newer(remote: &str, current: &str) -> bool {
    let (Ok(remote), Ok(current)) = (
        semver::Version::parse(remote),
        semver::Version::parse(current),
    ) else {
        return false;
    };
    remote.cmp_precedence(&current).is_gt()
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

#[cfg(test)]
mod tests {
    use super::remote_version_is_newer;

    #[test]
    fn update_available_when_remote_is_greater() {
        assert!(remote_version_is_newer("0.2.4", "0.2.3"));
    }

    #[test]
    fn update_unavailable_when_remote_is_equal() {
        assert!(!remote_version_is_newer("0.2.3", "0.2.3"));
    }

    #[test]
    fn update_unavailable_when_remote_is_older() {
        assert!(!remote_version_is_newer("0.2.2", "0.2.3"));
    }

    #[test]
    fn update_unavailable_when_remote_only_differs_as_string() {
        assert!(!remote_version_is_newer("0.2.3+build.1", "0.2.3"));
    }

    #[test]
    fn update_unavailable_when_version_is_invalid() {
        assert!(!remote_version_is_newer("not-a-version", "0.2.3"));
    }
}
