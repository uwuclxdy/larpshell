use colored::Colorize;
use inquire::Confirm;
use serde::Deserialize;
use std::fs;
use std::process::Command;

use crate::cli::{map_inquire_cancel, print_ok, print_warning, render_config};
use crate::common::{CTP_GREEN, CTP_YELLOW, clear_n_lines, show_cursor};
use crate::confirmation::style_message_markup;
use crate::error::LarpshellError;
use crate::shell_integration::remove_shell_integration;

#[derive(Deserialize)]
struct CargoManifest {
    package: Option<CargoPackage>,
}

#[derive(Deserialize)]
struct CargoPackage {
    name: String,
}

pub fn uninstall_larpshell() -> Result<(), LarpshellError> {
    eprintln!(
        "{}",
        style_message_markup("uninstalling larpshell...")
            .custom_color(CTP_YELLOW)
            .bold()
    );
    eprintln!();

    handle_shell_integration();
    uninstall_cargo_crate()?;
    remove_config_optional()?;
    remove_repo_optional()?;

    eprintln!();
    eprintln!(
        "{}",
        style_message_markup("larpshell successfully uninstalled.")
            .custom_color(CTP_GREEN)
            .bold()
    );
    eprintln!(
        "{}",
        style_message_markup(
            "please restart your shell or run 'source ~/.bashrc' (or 'source ~/.config/fish/config.fish' for fish).",
        )
        .custom_color(CTP_YELLOW)
    );

    Ok(())
}

fn handle_shell_integration() {
    match remove_shell_integration() {
        Ok(true) => print_ok("removed shell integration"),
        Ok(false) => eprintln!(
            "{}",
            style_message_markup("  no shell integration found").dimmed()
        ),
        Err(e) => print_warning(&format!("failed to remove shell integration: {e}")),
    }
}

fn uninstall_cargo_crate() -> Result<(), LarpshellError> {
    let output = Command::new("cargo")
        .args(["uninstall", "larpshell"])
        .output()?;

    if output.status.success() {
        print_ok("uninstalled cargo crate");
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("package 'larpshell' is not installed")
            || stderr.contains("not installed")
        {
            eprintln!(
                "{}",
                style_message_markup("  cargo crate not installed").dimmed()
            );
        } else {
            let trimmed = stderr.trim();
            print_warning(&format!("failed to uninstall: {trimmed}"));
        }
    }
    Ok(())
}

fn remove_config_optional() -> Result<(), LarpshellError> {
    eprintln!();
    show_cursor();
    let remove_config = Confirm::new("Remove configuration?")
        .with_default(false)
        .with_render_config(render_config())
        .prompt()
        .map_err(map_inquire_cancel)?;
    // Erase the persisted answer line itself, not just the blank line below it.
    clear_n_lines(2);

    if remove_config {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| {
                LarpshellError::ConfigError("failed to get config directory".to_string())
            })?
            .join("larpshell");

        if config_dir.exists() {
            fs::remove_dir_all(&config_dir)?;
            print_ok("removed configuration");
        } else {
            eprintln!(
                "{}",
                style_message_markup("  no configuration found").dimmed()
            );
        }
    }
    Ok(())
}

fn is_larpshell_manifest(contents: &str) -> Result<bool, LarpshellError> {
    let manifest: CargoManifest = toml::from_str(contents)?;
    Ok(manifest
        .package
        .is_some_and(|package| package.name == "larpshell"))
}

fn remove_repo_optional() -> Result<(), LarpshellError> {
    let current_dir = std::env::current_dir()?;
    let cargo_toml = current_dir.join("Cargo.toml");

    if cargo_toml.exists() {
        let contents = fs::read_to_string(&cargo_toml)?;
        if is_larpshell_manifest(&contents)? {
            eprintln!();
            show_cursor();
            let remove_repo = Confirm::new("Remove current directory (larpshell repository)?")
                .with_default(false)
                .with_render_config(render_config())
                .prompt()
                .map_err(map_inquire_cancel)?;
            clear_n_lines(2);

            if remove_repo {
                eprintln!(
                    "{}",
                    style_message_markup("  removing directory...").dimmed()
                );
                let parent = current_dir.parent().ok_or_else(|| {
                    LarpshellError::ConfigError("cannot remove root directory".to_string())
                })?;
                std::env::set_current_dir(parent)?;
                fs::remove_dir_all(&current_dir)?;
                print_ok("removed larpshell repository");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn larpshell_manifest_requires_package_name() {
        assert!(is_larpshell_manifest("[package]\nname = \"larpshell\"\n").unwrap());
    }

    #[test]
    fn larpshell_manifest_ignores_matching_dependency_name() {
        let manifest = "[package]\nname = \"other\"\n\n[dependencies]\nlarpshell = \"1\"\n";
        assert!(!is_larpshell_manifest(manifest).unwrap());
    }

    #[test]
    fn larpshell_manifest_ignores_matching_string_outside_package() {
        let manifest = "[workspace.package]\nname = \"larpshell\"\n";
        assert!(!is_larpshell_manifest(manifest).unwrap());
    }
}
