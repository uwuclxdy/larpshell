use colored::Colorize;
use inquire::Confirm;
use std::fs;
use std::process::Command;

use crate::cli::{print_ok, print_warning};
use crate::common::{CTP_GREEN, CTP_YELLOW, clear_line, show_cursor};
use crate::error::LarpshellError;
use crate::shell_integration::remove_shell_integration;

pub fn uninstall_larpshell() -> Result<(), LarpshellError> {
    eprintln!(
        "{}",
        "uninstalling larpshell...".custom_color(CTP_YELLOW).bold()
    );
    eprintln!();

    handle_shell_integration();
    uninstall_cargo_crate()?;
    remove_config_optional()?;
    remove_repo_optional()?;

    eprintln!();
    eprintln!(
        "{}",
        "larpshell successfully uninstalled."
            .custom_color(CTP_GREEN)
            .bold()
    );
    eprintln!("{}", "please restart your shell or run 'source ~/.bashrc' (or 'source ~/.config/fish/config.fish' for fish).".custom_color(CTP_YELLOW));

    Ok(())
}

fn handle_shell_integration() {
    match remove_shell_integration() {
        Ok(true) => print_ok("removed shell integration"),
        Ok(false) => eprintln!("{}", "  no shell integration found".dimmed()),
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
            eprintln!("{}", "  cargo crate not installed".dimmed());
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
        .prompt()
        .map_err(LarpshellError::InquireError)?;
    clear_line();

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
            eprintln!("{}", "  no configuration found".dimmed());
        }
    }
    Ok(())
}

fn remove_repo_optional() -> Result<(), LarpshellError> {
    let current_dir = std::env::current_dir()?;
    let cargo_toml = current_dir.join("Cargo.toml");

    if cargo_toml.exists() {
        let contents = fs::read_to_string(&cargo_toml)?;
        if contents.contains("name = \"larpshell\"") {
            eprintln!();
            show_cursor();
            let remove_repo = Confirm::new("Remove current directory (larpshell repository)?")
                .with_default(false)
                .prompt()
                .map_err(LarpshellError::InquireError)?;
            clear_line();

            if remove_repo {
                eprintln!("{}", "  removing directory...".dimmed());
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
